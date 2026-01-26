fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(false)
        .build_client(true)
        // Disable auto-generated connect method since it conflicts with our RPC name
        .build_transport(false)
        .compile_protos(&["../proto/agent.proto"], &["../proto"])?;
    Ok(())
}
