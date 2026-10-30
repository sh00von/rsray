//! Real JavaScript parsing with SWC, as a second extraction pass.
//!
//! The fast tokenizer in extractor.rs handles most bundles, but it
//! can miss or mis-split strings in minified code, nested templates
//! and dynamic imports. SWC parses the code for real, so its string
//! literals and template literals are exact. If SWC cannot parse the
//! input (TypeScript-only syntax, a fragment, a non-JS file), this
//! pass simply returns nothing and the tokenizer pass still runs.

use swc_common::{sync::Lrc, FileName, SourceMap};
use swc_ecma_ast::{Expr, Str, Tpl};
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax};
use swc_ecma_visit::{Visit, VisitWith};

#[derive(Default)]
struct Collector {
    values: Vec<String>,
}

impl Visit for Collector {
    fn visit_str(&mut self, s: &Str) {
        self.values.push(s.value.as_str().unwrap_or_default().to_string());
    }

    fn visit_tpl(&mut self, tpl: &Tpl) {
        // Rebuild the template: cooked quasi text, with each
        // expression replaced by {name} or {param}, matching the
        // tokenizer pass so the two passes deduplicate cleanly.
        let mut out = String::new();
        for (i, quasi) in tpl.quasis.iter().enumerate() {
            if let Some(cooked) = &quasi.cooked {
                if let Some(c) = cooked.as_str() {
                    out.push_str(c);
                }
            } else {
                out.push_str(quasi.raw.as_str());
            }
            if i < tpl.exprs.len() {
                match &*tpl.exprs[i] {
                    Expr::Ident(ident) => {
                        out.push('{');
                        out.push_str(ident.sym.as_str());
                        out.push('}');
                    }
                    _ => out.push_str("{param}"),
                }
            }
        }
        if !out.is_empty() {
            self.values.push(out);
        }
        // Still visit the expressions, so a string literal inside
        // an expression is not lost.
        tpl.exprs.visit_with(self);
    }
}

/// Parse `src` with SWC and return every string literal and template
/// literal value found. Returns an empty vec on any parse failure.
pub fn extract_swc(src: &str) -> Vec<String> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(Lrc::new(FileName::Anon), src.to_string());
    let lexer = Lexer::new(
        Syntax::Es(swc_ecma_parser::EsSyntax {
            jsx: true,
            ..Default::default()
        }),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    let program = match parser.parse_program() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    let mut collector = Collector::default();
    program.visit_with(&mut collector);
    collector.values
}
