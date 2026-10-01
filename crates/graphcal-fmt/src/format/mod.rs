mod decl;
mod doc;
mod expr;
mod formatter;
mod unit_dim;

use graphcal_compiler::syntax::ast::File;
use graphcal_compiler::syntax::comments::SourceMetadata;
use pretty::RcDoc;

use decl::format_decl_sequence;
use formatter::Formatter;

pub fn format_file(file: &File, source: &str, metadata: &SourceMetadata) -> RcDoc<'static> {
    let mut fmt = Formatter::new(source, metadata);
    let mut docs = format_decl_sequence(&mut fmt, &file.declarations);

    // Drain any remaining comments at end of file. `drain_comments_before`
    // already appends a hardline after every emitted comment, so that hardline
    // is the final newline when trailing comments exist.
    let had_remaining = fmt
        .drain_comments_before(usize::MAX)
        .is_some_and(|remaining| {
            if !docs.is_empty() {
                docs.push(RcDoc::hardline());
            }
            docs.push(remaining);
            true
        });

    if !had_remaining {
        docs.push(RcDoc::hardline());
    }

    RcDoc::concat(docs)
}
