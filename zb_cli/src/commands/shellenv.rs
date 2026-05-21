use std::path::Path;

pub fn execute(root: &Path, prefix: &Path, shell: Option<String>) -> Result<(), zb_core::Error> {
    print!("{}", render_shellenv(root, prefix, shell.as_deref()));
    Ok(())
}

fn render_shellenv(root: &Path, prefix: &Path, shell: Option<&str>) -> String {
    let shell_name = shell
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| std::env::var("SHELL").ok().and_then(last_path_component))
        .unwrap_or_else(|| "sh".to_string());

    let cellar = prefix.join("Cellar");
    match shell_name.as_str() {
        "fish" | "-fish" => render_fish(root, prefix, &cellar),
        "csh" | "-csh" | "tcsh" | "-tcsh" => render_csh(root, prefix, &cellar),
        _ => render_posix(
            root,
            prefix,
            &cellar,
            shell_name == "zsh" || shell_name == "-zsh",
        ),
    }
}

fn render_posix(root: &Path, prefix: &Path, cellar: &Path, include_fpath: bool) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "export HOMEBREW_PREFIX=\"{}\";\n",
        prefix.display()
    ));
    output.push_str(&format!(
        "export HOMEBREW_CELLAR=\"{}\";\n",
        cellar.display()
    ));
    output.push_str(&format!(
        "export HOMEBREW_REPOSITORY=\"{}\";\n",
        root.display()
    ));
    if include_fpath {
        output.push_str(&format!(
            "fpath[1,0]=\"{}/share/zsh/site-functions\";\nexport FPATH;\n",
            prefix.display()
        ));
    }
    output.push_str(&format!(
        "export PATH=\"{}/bin:{}/sbin${{PATH+:$PATH}}\";\n",
        prefix.display(),
        prefix.display()
    ));
    output.push_str("[ -z \"${MANPATH-}\" ] || export MANPATH=\":${MANPATH#:}\";\n");
    output.push_str(&format!(
        "export INFOPATH=\"{}/share/info:${{INFOPATH:-}}\";\n",
        prefix.display()
    ));
    output
}

fn render_fish(root: &Path, prefix: &Path, cellar: &Path) -> String {
    format!(
        "set --global --export HOMEBREW_PREFIX \"{}\";\n\
         set --global --export HOMEBREW_CELLAR \"{}\";\n\
         set --global --export HOMEBREW_REPOSITORY \"{}\";\n\
         fish_add_path --global --move --path \"{}/bin\" \"{}/sbin\";\n\
         if test -n \"$MANPATH[1]\"; set --global --export MANPATH '' $MANPATH; end;\n\
         if not contains \"{}/share/info\" $INFOPATH; set --global --export INFOPATH \"{}/share/info\" $INFOPATH; end;\n",
        prefix.display(),
        cellar.display(),
        root.display(),
        prefix.display(),
        prefix.display(),
        prefix.display(),
        prefix.display()
    )
}

fn render_csh(root: &Path, prefix: &Path, cellar: &Path) -> String {
    format!(
        "setenv HOMEBREW_PREFIX {};\n\
         setenv HOMEBREW_CELLAR {};\n\
         setenv HOMEBREW_REPOSITORY {};\n\
         setenv PATH {}/bin:{}/sbin:$PATH;\n\
         test ${{?MANPATH}} -eq 1 && setenv MANPATH :${{MANPATH}};\n\
         setenv INFOPATH {}/share/info`test ${{?INFOPATH}} -eq 1 && echo :${{INFOPATH}}`;\n",
        prefix.display(),
        cellar.display(),
        root.display(),
        prefix.display(),
        prefix.display(),
        prefix.display()
    )
}

fn last_path_component(path: String) -> Option<String> {
    std::path::Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::render_shellenv;

    #[test]
    fn renders_posix_exports() {
        let output = render_shellenv(
            Path::new("/tmp/zb-root"),
            Path::new("/tmp/zb-prefix"),
            Some("sh"),
        );

        assert!(output.contains("export HOMEBREW_PREFIX=\"/tmp/zb-prefix\";"));
        assert!(output.contains("export HOMEBREW_CELLAR=\"/tmp/zb-prefix/Cellar\";"));
        assert!(output.contains("export HOMEBREW_REPOSITORY=\"/tmp/zb-root\";"));
        assert!(output.contains("export PATH=\"/tmp/zb-prefix/bin:/tmp/zb-prefix/sbin"));
    }

    #[test]
    fn renders_fish_exports() {
        let output = render_shellenv(
            Path::new("/tmp/zb-root"),
            Path::new("/tmp/zb-prefix"),
            Some("fish"),
        );

        assert!(output.contains("set --global --export HOMEBREW_PREFIX"));
        assert!(output.contains("fish_add_path --global --move --path"));
    }
}
