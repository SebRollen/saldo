//! The book that describes saldo, here so that what's read of the language
//! is of the saldo that's built: a tool that teaches it, like a server that
//! gives it to Claude, reads it from this.

/// Each chapter, in the order the book's summary lists them: the name of its
/// file, its title, and its text.
pub const CHAPTERS: &[(&str, &str, &str)] = &[
    (
        "intro",
        "Introduction",
        include_str!("../book/src/intro.md"),
    ),
    (
        "simulation",
        "How a simulation runs",
        include_str!("../book/src/simulation.md"),
    ),
    (
        "accounts",
        "Accounts",
        include_str!("../book/src/accounts.md"),
    ),
    (
        "schedules",
        "Sets of days",
        include_str!("../book/src/schedules.md"),
    ),
    ("values", "Values", include_str!("../book/src/values.md")),
    ("entries", "Entries", include_str!("../book/src/entries.md")),
    ("checks", "Checks", include_str!("../book/src/checks.md")),
    ("fns", "Functions", include_str!("../book/src/fns.md")),
    ("modules", "Modules", include_str!("../book/src/modules.md")),
    (
        "distributions",
        "Random draws",
        include_str!("../book/src/distributions.md"),
    ),
    (
        "monte-carlo",
        "Many runs",
        include_str!("../book/src/monte-carlo.md"),
    ),
    (
        "scenarios",
        "Scenarios",
        include_str!("../book/src/scenarios.md"),
    ),
];

#[cfg(test)]
mod tests {
    #[test]
    fn every_chapter_of_the_summary_is_here() {
        // The summary's links, like `- [Accounts](./accounts.md)`.
        let listed: Vec<(String, String)> = include_str!("../book/src/SUMMARY.md")
            .lines()
            .filter_map(|line| {
                let (title, file) = line.trim().strip_suffix(')')?.split_once("](")?;
                let title = title.trim_start_matches(['-', '*', ' ', '[']);
                let name = file.trim_start_matches("./").trim_end_matches(".md");
                Some((name.to_string(), title.to_string()))
            })
            .collect();
        let here: Vec<(String, String)> = super::CHAPTERS
            .iter()
            .map(|(name, title, _)| (name.to_string(), title.to_string()))
            .collect();
        assert_eq!(here, listed);
    }
}
