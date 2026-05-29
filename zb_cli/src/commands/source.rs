pub async fn execute(
    installer: &mut zb_io::Installer,
    formulas: Vec<String>,
) -> Result<(), zb_core::Error> {
    for formula in formulas {
        let formula = installer.formula_metadata(&formula).await?;
        let source = formula
            .source_url()
            .map(|url| url.url.as_str())
            .unwrap_or("unknown");
        println!("{source}");
    }
    Ok(())
}
