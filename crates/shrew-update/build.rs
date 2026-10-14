//! Bakes the target triple in: it names the release asset this build updates itself from.

fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    println!("cargo:rustc-env=SHREW_UPDATE_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}
