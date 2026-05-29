use console::style;

pub async fn execute(
    installer: &mut zb_io::Installer,
    formulas: Vec<String>,
    include_build: bool,
) -> Result<(), zb_core::Error> {
    let multiple = formulas.len() > 1;
    for formula in formulas {
        let dependents = installer
            .formula_dependents(&formula, include_build)
            .await?;
        if multiple {
            println!("{}", style(&formula).bold());
        }
        for dependent in dependents {
            println!("{dependent}");
        }
    }

    Ok(())
}
