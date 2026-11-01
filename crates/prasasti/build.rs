// Embeds Info.plist in the binary so `cargo run` also knows the document types.
fn main() {
    let plist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/Info.plist");
    let plist = plist.canonicalize().expect("packaging/Info.plist exists");
    println!("cargo:rerun-if-changed={}", plist.display());
    println!(
        "cargo:rustc-link-arg-bin=prasasti=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        plist.display()
    );

    // The test binary gets its own bundle identity, so saving behaves as in the app.
    let tests = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/Info.plist");
    println!("cargo:rerun-if-changed={}", tests.display());
    println!("cargo:rustc-link-arg-tests=-Wl,-sectcreate,__TEXT,__info_plist,{}", tests.display());
}
