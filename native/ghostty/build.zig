const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    const module = b.createModule(.{
        .target = target,
        .optimize = optimize,
    });
    module.addCSourceFile(.{
        .file = b.path("src/smoke.c"),
        .flags = &.{"-std=c11"},
    });
    module.addCMacro("GHOSTTY_STATIC", "");

    // Ghostty's install step also builds an Apple xcframework, including iOS
    // slices, whenever xcodebuild is present. Mux needs only the macOS library.
    const ghostty = b.dependency("ghostty", .{
        .target = target,
        .optimize = optimize,
        .@"emit-xcframework" = false,
    });
    const ghostty_static = ghostty.artifact("ghostty-vt-static");
    module.linkLibrary(ghostty_static);

    // Ghostty intentionally exposes only the static VT artifact to dependent
    // Zig packages. Its install step still produces the supported shared
    // library, including all transitive SIMD dependencies, and its headers.
    // Every package installs into the one prefix, so running that step puts
    // them where the Rust FFI adapter looks.
    b.getInstallStep().dependOn(ghostty.builder.getInstallStep());

    const smoke = b.addExecutable(.{
        .name = "mux-ghostty-smoke",
        .root_module = module,
    });
    b.installArtifact(smoke);

    const run = b.addRunArtifact(smoke);
    run.step.dependOn(b.getInstallStep());
    b.step("run", "Build and run the libghostty-vt smoke test").dependOn(&run.step);
}
