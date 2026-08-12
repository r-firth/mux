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

    const ghostty = b.dependency("ghostty", .{
        .target = target,
        .optimize = optimize,
    });
    const ghostty_static = ghostty.artifact("ghostty-vt-static");
    module.linkLibrary(ghostty_static);

    // Ghostty intentionally exposes only the static VT artifact to dependent
    // Zig packages. Its install step still produces the supported shared
    // library, including all transitive SIMD dependencies. Copy that complete
    // installation into our prefix for the Rust FFI adapter.
    const ghostty_install = ghostty.builder.getInstallStep();
    const install_libraries = b.addInstallDirectory(.{
        .source_dir = .{ .cwd_relative = ghostty.builder.getInstallPath(.lib, "") },
        .install_dir = .lib,
        .install_subdir = "",
    });
    install_libraries.step.dependOn(ghostty_install);
    b.getInstallStep().dependOn(&install_libraries.step);

    // addInstallDirectory intentionally omits symlinks. Install the stable
    // linker name as a regular file so Cargo and packaged applications never
    // need to know Ghostty's internal versioned filename.
    const install_linker_library = b.addInstallFileWithDir(
        .{ .cwd_relative = ghostty.builder.getInstallPath(.lib, "libghostty-vt.dylib") },
        .lib,
        "libghostty-vt.dylib",
    );
    install_linker_library.step.dependOn(ghostty_install);
    b.getInstallStep().dependOn(&install_linker_library.step);

    const install_headers = b.addInstallDirectory(.{
        .source_dir = .{ .cwd_relative = ghostty.builder.getInstallPath(.header, "") },
        .install_dir = .header,
        .install_subdir = "",
    });
    install_headers.step.dependOn(ghostty_install);
    b.getInstallStep().dependOn(&install_headers.step);

    const smoke = b.addExecutable(.{
        .name = "mux-ghostty-smoke",
        .root_module = module,
    });
    b.installArtifact(smoke);

    const run = b.addRunArtifact(smoke);
    run.step.dependOn(b.getInstallStep());
    b.step("run", "Build and run the libghostty-vt smoke test").dependOn(&run.step);
}
