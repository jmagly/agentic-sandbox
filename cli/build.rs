fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(false)
        .build_client(true)
        // Disable auto-generated connect method since it conflicts with our RPC name
        .build_transport(false)
        .compile_protos(&["../proto/agent.proto"], &["../proto"])?;

    // Embed the short git SHA for `sandboxctl --version`. We re-run the
    // probe whenever HEAD moves so the binary's build label stays honest.
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=SANDBOXCTL_BUILD_SHA={}", sha);
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs/heads");
    Ok(())
}
