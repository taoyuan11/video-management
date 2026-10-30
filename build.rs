fn main() {
    println!("cargo:rerun-if-changed=assets/branding/windows.rc");
    println!("cargo:rerun-if-changed=assets/branding/frame-library.ico");

    let mut binaries = vec!["frame-library-updater"];
    if std::env::var_os("CARGO_FEATURE_GUI").is_some() {
        binaries.push("video-management");
    }
    // Branding is required: fail the build instead of shipping a generic icon.
    embed_resource::compile_for("assets/branding/windows.rc", binaries, embed_resource::NONE)
        .manifest_required()
        .expect("Cannot embed the Windows application icon");
}
