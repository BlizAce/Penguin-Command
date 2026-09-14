use anyhow::Result;
use portable_pty::CommandBuilder;
use std::path::PathBuf;

const OSC7: &str = r#"printf '\033]7;file://%s%s\007' "$HOSTNAME" "$PWD""#;

/// Build the CommandBuilder for the user's shell, with OSC7 cwd reporting injected.
pub fn build_shell_command(shell_override: Option<&str>) -> Result<CommandBuilder> {
    let (shell, args) = resolve_shell(shell_override);
    let name = shell.rsplit(['/', '\\']).next().unwrap_or(&shell).to_lowercase();

    let mut cmd = CommandBuilder::new(&shell);
    for a in &args {
        cmd.arg(a);
    }
    cmd.env("PC_SHELL_INTEGRATION", "1");

    if !args.iter().any(|a| a.contains("rcfile") || a.contains("init-command")) {
        match name.as_str() {
            "bash" => {
                let rc = write_tmp("pc_bash_rc.sh", &format!(
                    "[ -f \"$HOME/.bashrc\" ] && . \"$HOME/.bashrc\"\n\
                     __pc_osc7() {{ {OSC7}; }}\n\
                     PROMPT_COMMAND=\"__pc_osc7${{PROMPT_COMMAND:+; $PROMPT_COMMAND}}\"\n"
                ))?;
                cmd.arg("--rcfile");
                cmd.arg(&rc);
            }
            "zsh" => {
                let dir = std::env::temp_dir().join("penguin_zdotdir");
                std::fs::create_dir_all(&dir)?;
                for f in ["zshenv", "zprofile", "zshrc", "zlogin"] {
                    std::fs::write(
                        dir.join(format!(".{f}")),
                        format!(
                            "[[ -n ${{PC_ORIG_ZDOTDIR:-}} ]] && [[ -f $PC_ORIG_ZDOTDIR/.{f} ]] && . $PC_ORIG_ZDOTDIR/.{f}\n\
                             [[ -z ${{PC_ORIG_ZDOTDIR:-}} ]] && [[ -f $HOME/.{f} ]] && . $HOME/.{f}\n"
                        ),
                    )?;
                }
                std::fs::write(
                    dir.join(".zshrc"),
                    format!(
                        "[[ -n ${{PC_ORIG_ZDOTDIR:-}} ]] && [[ -f $PC_ORIG_ZDOTDIR/.zshrc ]] && . $PC_ORIG_ZDOTDIR/.zshrc\n\
                         [[ -z ${{PC_ORIG_ZDOTDIR:-}} ]] && [[ -f $HOME/.zshrc ]] && . $HOME/.zshrc\n\
                         autoload -Uz add-zsh-hook 2>/dev/null && add-zsh-hook precmd __pc_osc7 2>/dev/null\n\
                         __pc_osc7() {{ {OSC7}; }}\n"
                    ),
                )?;
                cmd.env("PC_ORIG_ZDOTDIR", std::env::var("ZDOTDIR").unwrap_or_else(|_| dirs::home_dir().unwrap_or_default().display().to_string()));
                cmd.env("ZDOTDIR", &dir);
            }
            "fish" => {
                let rc = write_tmp("pc_fish.fish", &format!(
                    "function __pc_osc7 --on-event fish_prompt\n {OSC7}\nend\n"
                ))?;
                cmd.arg("--init-command");
                cmd.arg(format!("source {}", rc.display()));
            }
            "pwsh" | "powershell" | "powershell.exe" | "pwsh.exe" => {
                let seq = "$e=[char]27; [Console]::Out.Write(\"{0}]7;file://{1}{2}{3}a\",$e,$env:COMPUTERNAME,(Get-Location).Path,$e)";
                cmd.args([
                    "-NoExit",
                    "-Command",
                    &format!(
                        ". $PROFILE -ErrorAction SilentlyContinue; function global:__pc_prompt {{ {seq}; 'PS ' }}; function prompt {{ __pc_prompt }}"
                    ),
                ]);
            }
            _ => {}
        }
    }
    Ok(cmd)
}

fn write_tmp(name: &str, body: &str) -> Result<PathBuf> {
    let p = std::env::temp_dir().join(name);
    std::fs::write(&p, body)?;
    Ok(p)
}

pub fn resolve_shell(override_shell: Option<&str>) -> (String, Vec<String>) {
    if let Some(s) = override_shell {
        return (s.to_string(), vec![]);
    }
    #[cfg(windows)]
    {
        ("powershell.exe".into(), vec![])
    }
    #[cfg(not(windows))]
    {
        match std::env::var("SHELL") {
            Ok(s) if !s.is_empty() => (s, vec![]),
            _ => ("/bin/sh".into(), vec![]),
        }
    }
}
