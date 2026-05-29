pub async fn execute(installer: &mut zb_io::Installer) -> Result<(), zb_core::Error> {
    for token in installer.list_cask_tokens().await? {
        println!("{token}");
    }

    Ok(())
}
