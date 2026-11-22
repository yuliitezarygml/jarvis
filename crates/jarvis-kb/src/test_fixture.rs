//! Маленькая фикстурная база для тестов ранжирования (не входит в сборку без `cfg(test)`).

use std::path::Path;

use crate::{KbWriter, NewChunk, SourceRow};

struct Doc {
    source: &'static str,
    path: &'static str,
    url: &'static str,
    title: &'static str,
    primary: &'static [&'static str],
    /// (heading_path, body, команды фрагмента)
    chunks: &'static [(&'static str, &'static str, &'static [&'static str])],
}

const DOCS: &[Doc] = &[
    Doc {
        source: "windows-commands",
        path: "WindowsServerDocs/administration/windows-commands/robocopy.md",
        url: "https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/robocopy",
        title: "robocopy",
        primary: &["robocopy"],
        chunks: &[
            ("", "Copies file data from one location to another. Robust file copy.", &["robocopy"]),
            ("Syntax", "robocopy <source> <destination> [<file>[ ...]] [<options>]", &["robocopy"]),
            (
                "Examples",
                "To mirror a directory tree and delete extra files in destination: robocopy C:\\src D:\\dst /mir",
                &["robocopy"],
            ),
        ],
    },
    Doc {
        source: "windows-commands",
        path: "WindowsServerDocs/administration/windows-commands/xcopy.md",
        url: "https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/xcopy",
        title: "xcopy",
        primary: &["xcopy"],
        chunks: &[
            ("", "Copies files and directories, including subdirectories.", &["xcopy"]),
            ("Remarks", "For mirroring and resilient copy use robocopy instead: robocopy a b /e", &["xcopy", "robocopy"]),
        ],
    },
    Doc {
        source: "powershell",
        path: "reference/7.6/Microsoft.PowerShell.Management/Get-Service.md",
        url: "https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.management/get-service?view=powershell-7.6",
        title: "Get-Service",
        primary: &["Get-Service"],
        chunks: &[
            ("", "Gets the services on the computer. Get-Service returns objects that represent services.", &["Get-Service"]),
            ("Examples > Example 1", "Get-Service | Where-Object Status -eq Running", &["Get-Service", "Where-Object"]),
        ],
    },
    Doc {
        source: "powershell",
        path: "reference/7.6/Microsoft.PowerShell.Management/Stop-Service.md",
        url: "https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.management/stop-service?view=powershell-7.6",
        title: "Stop-Service",
        primary: &["Stop-Service"],
        chunks: &[(
            "",
            "Stops one or more running services. You can pipe Get-Service output to Stop-Service.",
            &["Stop-Service", "Get-Service"],
        )],
    },
    Doc {
        source: "powershell",
        path: "docset/winserver2025-ps/NetSecurity/New-NetFirewallRule.md",
        url: "https://learn.microsoft.com/en-us/powershell/module/netsecurity/new-netfirewallrule",
        title: "New-NetFirewallRule",
        primary: &["New-NetFirewallRule"],
        chunks: &[(
            "",
            "Creates a new inbound or outbound firewall rule and adds the rule to the target computer.",
            &["New-NetFirewallRule"],
        )],
    },
    Doc {
        source: "windows-client",
        path: "windows/security/operating-system-security/network-security/windows-firewall/index.md",
        url: "https://learn.microsoft.com/en-us/windows/security/operating-system-security/network-security/windows-firewall/",
        title: "Windows Firewall overview",
        primary: &[],
        chunks: &[(
            "",
            "Windows Firewall is a security feature that filters network traffic. You can allow an app through \
             the firewall and create inbound rules.",
            &[],
        )],
    },
    Doc {
        source: "seed",
        path: "kb/seed/startup.md",
        url: "kb/seed/startup.md",
        title: "Автозагрузка программ (startup apps)",
        primary: &[],
        chunks: &[(
            "",
            "Как отключить автозагрузку программ: Диспетчер задач → Автозагрузка. Disable startup apps in Task \
             Manager or Settings > Apps > Startup.",
            &[],
        )],
    },
];

pub(crate) fn build(path: &Path) {
    let mut w = KbWriter::create(path).unwrap();
    for d in DOCS {
        for (i, (hp, body, cmds)) in d.chunks.iter().enumerate() {
            let cmds: Vec<String> = cmds.iter().map(|s| s.to_string()).collect();
            let heading = hp.rsplit(" > ").next().unwrap_or("");
            let id = w
                .add_chunk(&NewChunk {
                    source: d.source,
                    path: d.path,
                    url: d.url,
                    title: d.title,
                    heading,
                    heading_path: hp,
                    commands: &cmds,
                    body,
                    ord: i as u32,
                    ..Default::default()
                })
                .unwrap();
            for c in &cmds {
                let primary = i == 0 && d.primary.contains(&c.as_str());
                w.add_command(c, id, primary).unwrap();
            }
        }
    }
    for s in ["windows-commands", "powershell", "windows-client", "seed"] {
        w.set_source(&SourceRow { id: s.into(), name: s.into(), ..Default::default() }).unwrap();
    }
    w.finish().unwrap();
}

pub(crate) fn build_small(path: &Path) {
    let mut w = KbWriter::create(path).unwrap();
    w.add_chunk(&NewChunk { source: "seed", path: "x", url: "x", title: "x", body: "x", ..Default::default() })
        .unwrap();
    w.finish().unwrap();
}
