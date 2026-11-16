//! カーネルのリンカスクリプトを指定するビルドスクリプト。

use std::env;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
    let target = env::var("TARGET").expect("TARGET is set by cargo");

    if target == "x86_64-unknown-none" {
        println!("cargo:rustc-link-arg-bins=-T{manifest_dir}/linker.ld");
    } else {
        // ホスト向けにビルドしようとした場合は分かりやすく失敗させる
        println!("cargo:warning=yumekawa-kernel は x86_64-unknown-none 専用です (TARGET={target})");
    }
    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rerun-if-changed=build.rs");
}
