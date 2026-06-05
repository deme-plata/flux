// TeX escaping + small builder helpers. Markdown source text is user-ish
// content (titles, paragraphs) so it MUST be escaped before landing in the
// emitted document — otherwise a `_` in a filename or `&` in a sentence kills
// pdflatex with a cryptic "Missing $ inserted" error.

/// Escape every TeX special so a string can be embedded in a paragraph or
/// `\section{}` argument safely. Conservative — escapes more than strictly
/// required (e.g. `<` `>`) to dodge package-specific gotchas.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str(r"\textbackslash{}"),
            '{' => out.push_str(r"\{"),
            '}' => out.push_str(r"\}"),
            '$' => out.push_str(r"\$"),
            '&' => out.push_str(r"\&"),
            '#' => out.push_str(r"\#"),
            '%' => out.push_str(r"\%"),
            '_' => out.push_str(r"\_"),
            '^' => out.push_str(r"\textasciicircum{}"),
            '~' => out.push_str(r"\textasciitilde{}"),
            '<' => out.push_str(r"\textless{}"),
            '>' => out.push_str(r"\textgreater{}"),
            '|' => out.push_str(r"\textbar{}"),
            '"' => out.push_str(r"''"),
            // Common Greek + math + arrow glyphs that show up in our
            // markdown source corpus but blow up the lmodern font. Lower to
            // ASCII-named macros (rendered via `textgreek` / wrapped in
            // math mode). Unknown codepoints above U+00FF degrade to '?'
            // rather than producing "Missing character" fatals.
            c if c.is_ascii() => out.push(c),
            c => out.push_str(unicode_to_tex(c)),
        }
    }
    out
}

/// Map a non-ASCII codepoint to a pdfLaTeX-safe sequence. Greek letters use
/// the `textgreek` package's `\textXXX` macros; math symbols are wrapped in
/// `$...$`; common Latin-1 dashes / quotes / spaces use their standard
/// LaTeX commands. Anything not in the table degrades to `?`.
fn unicode_to_tex(c: char) -> &'static str {
    match c {
        // Latin-1 punctuation we don't otherwise touch.
        '\u{00A0}' => "~",
        '\u{2013}' => "--",                    // en dash
        '\u{2014}' => "---",                   // em dash
        '\u{2018}' => "`",                     // left single quote
        '\u{2019}' => "'",                     // right single quote
        '\u{201C}' => "``",                    // left double quote
        '\u{201D}' => "''",                    // right double quote
        '\u{2026}' => "\\ldots{}",            // …
        '\u{00B7}' => "$\\cdot$",             // middle dot
        '\u{00D7}' => "$\\times$",            // ×
        '\u{2192}' => "$\\rightarrow$",       // →
        '\u{2190}' => "$\\leftarrow$",        // ←
        '\u{2194}' => "$\\leftrightarrow$",   // ↔
        '\u{2248}' => "$\\approx$",           // ≈
        '\u{2260}' => "$\\neq$",              // ≠
        '\u{2264}' => "$\\leq$",              // ≤
        '\u{2265}' => "$\\geq$",              // ≥
        '\u{00B1}' => "$\\pm$",               // ±
        '\u{00B0}' => "$^\\circ$",            // °
        // Greek lowercase — needs `textgreek` package in the preamble.
        '\u{03B1}' => "\\textalpha{}",
        '\u{03B2}' => "\\textbeta{}",
        '\u{03B3}' => "\\textgamma{}",
        '\u{03B4}' => "\\textdelta{}",
        '\u{03B5}' => "\\textepsilon{}",
        '\u{03B6}' => "\\textzeta{}",
        '\u{03B7}' => "\\texteta{}",
        '\u{03B8}' => "\\texttheta{}",
        '\u{03BB}' => "\\textlambda{}",
        '\u{03BC}' => "\\textmu{}",
        '\u{03C0}' => "\\textpi{}",
        '\u{03C1}' => "\\textrho{}",
        '\u{03C3}' => "\\textsigma{}",
        '\u{03C4}' => "\\texttau{}",
        '\u{03C6}' => "\\textphi{}",
        '\u{03C7}' => "\\textchi{}",
        '\u{03C8}' => "\\textpsi{}",
        '\u{03C9}' => "\\textomega{}",
        // Greek uppercase.
        '\u{0394}' => "$\\Delta$",
        '\u{03A3}' => "$\\Sigma$",
        '\u{03A9}' => "$\\Omega$",
        '\u{03A0}' => "$\\Pi$",
        // Currency / misc that crops up in financial sections.
        '\u{20AC}' => "\\euro{}",
        '\u{00A3}' => "\\pounds{}",
        '\u{00A9}' => "\\textcopyright{}",
        '\u{00AE}' => "\\textregistered{}",
        '\u{2122}' => "\\texttrademark{}",
        '\u{2713}' => "$\\checkmark$",
        // Box-drawing + emoji + everything else: drop to `?`. Cleaner than
        // a "Missing character" warning + visible glyph dropout.
        _ => "?",
    }
}

/// Wrap a literal-ish value in `\texttt{}` — used for crate names, file paths,
/// task ids, anywhere monospace makes scanning easier.
pub fn mono(s: &str) -> String {
    format!("\\texttt{{{}}}", escape(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_common_tex_specials() {
        // The two specials most likely to land verbatim in a markdown title:
        // underscores in crate names, ampersands in sentences.
        assert_eq!(escape("flux_api & deepseek"), "flux\\_api \\& deepseek");
    }

    #[test]
    fn escape_handles_backslash() {
        // Backslash needs the `{}` to terminate the `\textbackslash` macro;
        // otherwise the next character gets swallowed.
        assert!(escape(r"a\b").contains(r"\textbackslash{}"));
    }

    #[test]
    fn mono_wraps_and_escapes() {
        let m = mono("flux_api");
        assert!(m.starts_with("\\texttt{"));
        assert!(m.contains("flux\\_api"));
    }

    #[test]
    fn dollar_sign_does_not_open_math() {
        // Plain "$100" should not start math mode in the emitted doc.
        let e = escape("$100 saved");
        assert!(e.starts_with("\\$"));
    }
}
