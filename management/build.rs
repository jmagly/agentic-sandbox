fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        // Rename Connect RPC to avoid conflict with tonic
        .build_transport(false)
        .compile_protos(&["../proto/agent.proto"], &["../proto"])?;
    Ok(())
}
