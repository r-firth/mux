// The ground: a panel of round dots, the same dots the session's name is
// set in, lit by the light `Field` works out in ground.rs. Every dot is
// always faintly there; the light brings dots up through an ordered dither,
// so a swell crossing the panel reads as a pattern shifting, not as noise.
// Names between dollar signs are the ground's own numbers, written in when
// the shader is built.
#include <metal_stdlib>
using namespace metal;

struct Frame {
    float4 view;
    float4 clock;
    float4 gain;
    float4 focus;
    float4 focus_light;
    float4 chip;
    float4 chip_light;
    float4 wake;
    float4 leaving;
    float4 counts;
    float4 blobs[8];
    float4 blob_inks[2];
    float4 busy[$most$];
    float4 busy_light[2];
    float4 calls[$most$];
    float4 rings[$most$];
    float4 ringing[2];
    float4 tones[16];
    float4 old_tones[16];
    float4 ground;
    float4 slabs[$slabs$];
};

struct Corner {
    float4 position [[position]];
};

vertex Corner ground_vertex(uint id [[vertex_id]]) {
    float2 at = float2((id << 1) & 2, id & 2);
    Corner corner;
    corner.position = float4(at * 2.0 - 1.0, 0.0, 1.0);
    return corner;
}

static float fall(float distance, float reach) {
    float near = max(1.0 - distance / reach, 0.0);
    return near * near;
}

// How far outside a rect (x, y, width, height) a point is.
static float outside(float2 point, float4 rect) {
    float2 away = max(max(rect.xy - point, point - rect.xy - rect.zw), float2(0.0));
    return length(away);
}

// How far a light of this strength brings up a dot whose turn comes at
// `threshold`: softly, as gofer's signal does, so a dot fades up rather than
// snapping on.
static float lit(float value, float threshold) {
    return smoothstep(threshold - 0.12, threshold + 0.12, value);
}

// The light a focused slab, the active chip, busy panes and a wake cast at a
// point, apart from the slow fields and the tide: what glows under the dots
// as well as lighting them.
static float cast(float2 point, constant Frame &frame, thread float &signal) {
    float own = 0.0;
    if (frame.wake.w > 0.0) {
        float behind = frame.wake.z - distance(point, frame.wake.xy);
        if (behind >= 0.0 && behind < $wake_trail$) {
            float tail = 1.0 - behind / $wake_trail$;
            own += frame.wake.w * tail * tail * smoothstep(0.0, 8.0, behind);
        }
    }
    if (frame.focus_light.y > 0.5) {
        float away = outside(point, frame.focus);
        own += frame.focus_light.x
            * (0.42 * fall(away, $focus_reach$) + 0.18 * fall(away, $focus_rim$));
    }
    if (frame.chip_light.x > 0.5) {
        own += 0.3 * fall(outside(point, frame.chip), $chip_glow$);
    }
    for (int index = 0; index < int(frame.counts.x); index++) {
        own += frame.busy_light[index / 4][index % 4] * 0.45
            * fall(outside(point, frame.busy[index]), $busy_reach$);
    }
    for (int index = 0; index < int(frame.counts.y); index++) {
        float away = outside(point, frame.calls[index]);
        signal += 0.3 * fall(away, 20.0);
        if (frame.ringing[index / 4][index % 4] > 0.5) {
            float4 rings = frame.rings[index];
            float near = max(1.0 - abs(away - rings.x) / $ring_width$, 0.0);
            float far = max(1.0 - abs(away - rings.z) / $ring_width$, 0.0);
            signal += rings.y * near * near + rings.w * far * far;
        } else {
            signal += 0.3 * fall(away, 48.0);
        }
    }
    return own;
}

fragment float4 ground_fragment(
    Corner in [[stage_in]],
    constant Frame &frame [[buffer(0)]],
    texture2d<float> noise [[texture(0)]]
) {
    const float turn = 6.28318530718;
    const float pitch = $pitch$;
    float scale = frame.clock.w;
    float2 here = in.position.xy / scale;
    // Under a slab nothing of the ground shows: leave it dark and save the
    // work. A slab's rounded corners do show a little, so they are lit.
    for (int index = 0; index < int(frame.counts.z); index++) {
        float4 slab = frame.slabs[index];
        float2 into = here - slab.xy;
        float2 left = slab.zw - into;
        bool inside = into.x > 0.0 && into.y > 0.0 && left.x > 0.0 && left.y > 0.0;
        bool cornered = min(into.x, left.x) < $corner$ && min(into.y, left.y) < $corner$;
        if (inside && !cornered) {
            return float4(frame.ground.rgb, 1.0);
        }
    }
    uint2 at = uint2(here / pitch);
    // The middle of the dot this pixel belongs to: the light is worked out
    // there, so a dot is one colour all over.
    float2 point = (float2(at) + 0.5) * pitch;

    // The slow fields.
    float fields[3] = { 0.0, 0.0, 0.0 };
    for (int index = 0; index < 8; index++) {
        float4 blob = frame.blobs[index];
        float2 away = (point - blob.xy) / blob.z;
        int ink = int(frame.blob_inks[index / 4][index % 4]);
        fields[ink] += blob.w * exp(-dot(away, away) * 2.3);
    }
    float own = (frame.gain.w + fields[0]) * frame.gain.x;
    float first = fields[1] * frame.gain.y;
    float second = fields[2] * frame.gain.z;
    float signal = 0.0;

    // The tide carries the ground's light.
    bool moving = frame.clock.y > 0.5;
    if (moving) {
        float time = frame.clock.x;
        float reach = point.y < frame.view.w ? $tide_in_strip$ : 1.0;
        float wave = ((point.x * 0.86 + point.y * 0.5) / $wave_apart$ - time * $wave_pace$) * turn;
        float cross = ((point.y * 0.86 - point.x * 0.5) / $cross_apart$ - time * $cross_pace$) * turn;
        float crest = 0.5 + 0.5 * sin(wave);
        float swell = crest * crest * (0.6 + 0.4 * sin(cross));
        float carried = $trough$ + (1.6 - $trough$) * swell;
        own = own * (1.0 + (carried - 1.0) * reach) + $crest$ * swell * reach;
    }
    own += cast(point, frame, signal);

    // No two dots are alike: each has its own turn to come up, its own size
    // and its own brightness, read from a tile of noise, so the panel is
    // never even. The light then decides how much of it shows at all.
    float turn_own = (noise.read(at % 64).r * 255.0 + 0.5) / 256.0;
    float turn_first = (noise.read((at + uint2(23, 41)) % 64).r * 255.0 + 0.5) / 256.0;
    float turn_second = (noise.read((at + uint2(47, 13)) % 64).r * 255.0 + 0.5) / 256.0;
    float turn_signal = (noise.read((at + uint2(31, 29)) % 64).r * 255.0 + 0.5) / 256.0;
    float quirk = noise.read((at + uint2(11, 53)) % 64).r;

    // Each step a different few dots come up a little early.
    if (moving && ((uint(turn_signal * 256.0) + uint(frame.clock.z)) & 255) < $shimmer_share$) {
        own += $shimmer$;
    }

    // Sharpen the light so there are dark reaches and bright ones, not an
    // even glow.
    float strong = own * own * (3.0 - 2.0 * min(own, 1.0)) * $contrast$;
    float up[4] = {
        lit(strong, turn_own),
        lit(first * $accent$, turn_first),
        lit(second * $accent$, turn_second),
        lit(signal, turn_signal),
    };
    // Light past a dot's turn makes it burn brighter.
    float burn = lit(strong - 1.0, turn_own);

    // An ink dissolving away keeps the dots the new one has not reached,
    // its edge frayed by each dot's turn.
    bool old = false;
    if (frame.leaving.w > 0.5) {
        float reached = frame.leaving.z;
        float away = distance(point, frame.leaving.xy);
        old = away + (turn_own - 0.5) * $fray$ * 2.0 > reached;
    }

    // The dot. One the light has not reached is only there where there is
    // some light about: in the dark reaches the panel falls away to nothing.
    float about = smoothstep(0.02, 0.35, strong);
    float3 own_ink = old ? frame.old_tones[3].rgb : frame.tones[3].rgb;
    float3 glow = $unlit$ * about * (0.5 + quirk) * own_ink;
    glow += up[0] * (0.45 + 0.5 * quirk + 0.45 * burn) * own_ink;
    glow += up[1] * (old ? frame.old_tones[7].rgb : frame.tones[7].rgb);
    glow += up[2] * (old ? frame.old_tones[11].rgb : frame.tones[11].rgb);
    glow += up[3] * frame.tones[15].rgb;

    // Round, with a soft edge a pixel wide, and as big as it is bright: a
    // dim dot is a speck and a burning one nearly touches its neighbours.
    float most = max(max(up[0], up[1]), max(up[2], up[3]));
    float size = $dot_size$ * (0.55 + 0.45 * quirk)
        + $dot_swell$ * most * (0.4 + 0.6 * min(strong, 1.5) / 1.5);
    float radius = pitch * size;
    float cover = clamp((radius - distance(here, point)) * scale + 0.5, 0.0, 1.0);

    // Under the dots, the light a focused slab casts glows on the ground
    // itself, smoothly: worked out at the pixel, not the dot.
    float spare = 0.0;
    float wash = cast(here, frame, spare);
    float3 colour = frame.ground.rgb + $wash$ * min(wash, 1.2) * own_ink;

    colour = mix(colour, min(colour + glow, float3(1.0)), cover);
    return float4(colour, 1.0);
}
