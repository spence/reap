use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
  println!("cargo:rerun-if-changed=src/control_xpc.cpp");
  let target = env::var("TARGET").expect("Cargo target");
  if !target.ends_with("apple-darwin") {
    return;
  }
  let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));
  let object = output.join("control_xpc.o");
  let archive = output.join("libreap_control_xpc.a");
  let architecture = if target.starts_with("aarch64") {
    "arm64"
  } else {
    "x86_64"
  };
  let compiled = Command::new("/usr/bin/clang++")
    .args([
      "-std=c++17",
      "-fblocks",
      "-O2",
      "-Wall",
      "-Wextra",
      "-Werror",
      "-arch",
    ])
    .arg(architecture)
    .args(["-c", "src/control_xpc.cpp", "-o"])
    .arg(&object)
    .status()
    .expect("start the native XPC bridge compiler");
  assert!(compiled.success(), "native XPC bridge compilation failed");
  let archived = Command::new("/usr/bin/ar")
    .arg("crs")
    .arg(&archive)
    .arg(&object)
    .status()
    .expect("start the native archiver");
  assert!(archived.success(), "native XPC bridge archive failed");
  println!("cargo:rustc-link-search=native={}", output.display());
  println!("cargo:rustc-link-lib=static=reap_control_xpc");
  println!("cargo:rustc-link-lib=dylib=c++");
}
