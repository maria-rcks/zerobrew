use console::style;

pub async fn execute(
    installer: &mut zb_io::Installer,
    text: Vec<String>,
    formula: bool,
    cask: bool,
    name: bool,
    all: bool,
    desc: bool,
) -> Result<(), zb_core::Error> {
    if cask && !formula {
        return Err(zb_core::Error::UnsupportedFormula {
            name: text.join(" "),
            reason: "cask search is not supported yet".to_string(),
        });
    }

    let query = text.join(" ");
    let results = if name || all || desc {
        installer
            .search_formula_index(&query, name && !all && !desc)
            .await?
    } else {
        installer.suggest_formulas(&query, 20).await?
    };

    if results.is_empty() {
        return Err(zb_core::Error::MissingFormula { name: query });
    }

    println!("{}", style("Formulae").cyan().bold());
    for result in results {
        println!("{result}");
    }

    Ok(())
}
