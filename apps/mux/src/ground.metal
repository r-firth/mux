// The ground's light, cell for cell as `Field` works it out in ground.rs.
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

static float dot_level(float value, float threshold) {
    return float(value > threshold) + float(value > threshold + 1.0)
        + float(value > threshold + 2.0);
}

fragment float4 ground_fragment(
    Corner in [[stage_in]],
    constant Frame &frame [[buffer(0)]],
    texture2d<float> noise [[texture(0)]]
) {
    const uint2 corners[4] = { $corners$ };
    const float turn = 6.28318530718;
    float cell = frame.view.z;
    uint2 at = uint2(in.position.xy);
    float2 point = (float2(at) + 0.5) * cell;

    // Each ink reads the noise from its own corner of the tile.
    float raw[4];
    float threshold[4];
    for (int ink = 0; ink < 4; ink++) {
        raw[ink] = noise.read((at + corners[ink]) % 64).r * 255.0;
        threshold[ink] = (raw[ink] + 0.5) / 256.0;
    }

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

    // A wake passing.
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

    // Each step a different few dots light a little early.
    if (moving && ((uint(raw[3] + 0.5) + uint(frame.clock.z)) & 255) < $shimmer_share$) {
        own += $shimmer$;
    }

    int levels[4] = {
        int(dot_level(own, threshold[0])),
        int(dot_level(first, threshold[1])),
        int(dot_level(second, threshold[2])),
        int(dot_level(signal, threshold[3])),
    };
    bool lit = (levels[0] + levels[1] + levels[2] + levels[3]) > 0;

    // An ink dissolving away keeps the cells the new one has not reached,
    // its edge frayed by each cell's threshold.
    bool old = false;
    if (lit && frame.leaving.w > 0.5) {
        float reached = frame.leaving.z;
        float away = distance(point, frame.leaving.xy);
        bool inside = reached > $fray$ && away <= reached - $fray$;
        old = !inside
            && (away > reached + $fray$
                || away + (threshold[0] - 0.5) * $fray$ * 2.0 > reached);
    }

    float3 colour = frame.ground.rgb;
    for (int ink = 0; ink < 4; ink++) {
        int tone = ink * 4 + levels[ink];
        colour += old ? frame.old_tones[tone].rgb : frame.tones[tone].rgb;
    }
    return float4(min(colour, float3(1.0)), 1.0);
}
