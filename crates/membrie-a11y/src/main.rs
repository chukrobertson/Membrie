// SPDX-License-Identifier: AGPL-3.0-or-later

use anyhow::{Result, bail};
use membrie_a11y::inspect_active_window;

fn main() -> Result<()> {
    let show_text = std::env::args().any(|argument| argument == "--show-text");
    if !std::env::args().any(|argument| argument == "--consent") {
        bail!(
            "This read-only probe inspects the active application's accessibility tree. \
             It stores nothing and performs no actions. Run it again with --consent; add \
             --show-text only if you also want a local terminal preview of visible labels."
        );
    }

    println!("Membrie semantic compatibility probe");
    println!("Read only · no actions · no database writes");
    if show_text {
        println!("Visible label preview explicitly enabled");
    } else {
        println!("Visible label preview disabled");
    }

    let summary = inspect_active_window(show_text)?;
    println!("Application: {}", display_or_unknown(&summary.application));
    println!("Active window: {}", display_or_unknown(&summary.window));
    println!(
        "Coverage: {} · {} visible nodes · {} text-capable · {} document-capable",
        summary.quality(),
        summary.visible_nodes,
        summary.text_nodes,
        summary.document_nodes
    );
    println!("Traversal bounded at {} nodes", summary.nodes_seen);
    if show_text {
        println!("Visible semantic labels:");
        for line in &summary.preview {
            println!("  {line}");
        }
    }
    Ok(())
}

fn display_or_unknown(value: &str) -> &str {
    if value.trim().is_empty() {
        "Unknown"
    } else {
        value
    }
}
