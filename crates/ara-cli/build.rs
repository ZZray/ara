fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS");
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV");
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH");
    if matches!(
        (target_os.as_deref(), target_env.as_deref(), target_arch.as_deref()),
        (Ok("windows"), Ok("msvc"), Ok("x86_64"))
    ) {
        // Reserve virtual address space for the reference Host's async command
        // path; retain the existing 4 KiB initial commit and worker settings.
        println!("cargo::rustc-link-arg-bin=ara=/STACK:8388608,4096");
    }
}
