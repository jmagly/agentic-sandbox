fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(false) // Client only
        .build_client(true)
        // Disable transport layer to avoid conflict between tonic's connect<D>()
        // and the RPC Connect() method - we'll create the channel manually
        .build_transport(false)
        .compile_protos(&["../proto/agent.proto"], &["../proto"])?;
    Ok(())
}
