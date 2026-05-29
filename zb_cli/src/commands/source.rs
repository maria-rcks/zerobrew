pub async fn execute(
    installer: &mut zb_io::Installer,
    formulas: Vec<String>,
) -> Result<(), zb_core::Error> {
    for formula in formulas {
        print!("{}", installer.formula_source(&formula).await?);
    }
    Ok(())
}
