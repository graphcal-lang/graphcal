use crate::syntax::ast::{AssertBody, BinOp, DeclKind, Expr, ExprKind, File, TupleMatchArm};
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::Span;

// Desugaring: TupleMatch → nested If / BinOp(Eq)
// ---------------------------------------------------------------------------

/// Desugar all `TupleMatch` nodes in a file to nested `If`/`BinOp(Eq)` chains.
///
/// This must be called before evaluation, dim-checking, and dependency analysis,
/// which only understand the desugared form. The formatter and LSP symbol table
/// operate on the original AST (before desugaring) so they see `TupleMatch`.
///
/// Runs *after* [`crate::syntax::desugar::desugar_multi_decls_in_file`] —
/// hence the [`crate::syntax::phase::Desugared`] phase parameter.
pub fn desugar_tuple_matches(file: &mut File<crate::syntax::phase::Desugared>) {
    for decl in &mut file.declarations {
        match &mut decl.kind {
            DeclKind::Param(p) => {
                if let Some(v) = &mut p.value {
                    desugar_expr(v);
                }
            }
            DeclKind::Node(n) => desugar_expr(&mut n.value),
            DeclKind::ConstNode(c) => desugar_expr(&mut c.value),
            DeclKind::Unit(u) => {
                if let Some(def) = &mut u.definition {
                    desugar_expr(&mut def.scale_expr);
                }
            }
            DeclKind::Assert(a) => match &mut a.body {
                AssertBody::Expr(e) => desugar_expr(e),
                AssertBody::Tolerance {
                    actual,
                    expected,
                    tolerance,
                    ..
                } => {
                    desugar_expr(actual);
                    desugar_expr(expected);
                    desugar_expr(tolerance);
                }
            },
            DeclKind::Plot(p) => {
                for encoding in &mut p.encodings {
                    desugar_expr(&mut encoding.value);
                }
                for prop in &mut p.mark.properties {
                    desugar_expr(&mut prop.value);
                }
                for prop in &mut p.properties {
                    desugar_expr(&mut prop.value);
                }
            }
            DeclKind::Figure(f) => {
                for field in &mut f.fields {
                    desugar_expr(&mut field.value);
                }
            }
            DeclKind::Layer(l) => {
                for field in &mut l.fields {
                    desugar_expr(&mut field.value);
                }
            }
            DeclKind::Dag(d) => {
                // Recursively desugar declarations inside the dag block
                let mut inner_file = File::<crate::syntax::phase::Desugared> {
                    declarations: std::mem::take(&mut d.body),
                };
                desugar_tuple_matches(&mut inner_file);
                d.body = inner_file.declarations;
            }
            DeclKind::BaseDimension(_)
            | DeclKind::Dimension(_)
            | DeclKind::Index(_)
            | DeclKind::Type(_)
            | DeclKind::Import(_)
            | DeclKind::Include(_) => {}
            DeclKind::Sugar(_) => crate::syntax::desugar::unreachable_post_desugar(),
        }
    }
}

/// Recursively desugar `TupleMatch` inside a single expression.
fn desugar_expr(expr: &mut Expr<crate::syntax::phase::Desugared>) {
    // First, recurse into children.
    match &mut expr.kind {
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::TypeSystemRef(_)
        | ExprKind::UnitLiteral { .. }
        | ExprKind::LocalRef(_)
        | ExprKind::GraphRef(_)
        | ExprKind::ConstRef(_)
        | ExprKind::VariantLiteral { .. }
        | ExprKind::UnresolvedRef(_)
        // TupleMatch is handled below after recursing into children.
        | ExprKind::TupleMatch { .. } => {}
        ExprKind::InlineDagRef { args, .. } => {
            for b in args {
                desugar_expr(&mut b.value);
            }
        }
        ExprKind::BinOp { lhs, rhs, .. } => {
            desugar_expr(lhs);
            desugar_expr(rhs);
        }
        ExprKind::UnaryOp { operand, .. } => desugar_expr(operand),
        ExprKind::FnCall { args, .. } => {
            for a in args {
                desugar_expr(a);
            }
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            desugar_expr(condition);
            desugar_expr(then_branch);
            desugar_expr(else_branch);
        }
        ExprKind::Convert { expr: inner, .. }
        | ExprKind::DisplayTimezone { expr: inner, .. }
        | ExprKind::FieldAccess { expr: inner, .. }
        | ExprKind::IndexAccess { expr: inner, .. } => desugar_expr(inner),
        ExprKind::ConstructorCall { fields, .. } => {
            for f in fields {
                desugar_expr(&mut f.value);
            }
        }
        ExprKind::MapLiteral { entries } => {
            for e in entries {
                desugar_expr(&mut e.value);
            }
        }
        ExprKind::ForComp { body, .. } => desugar_expr(body),
        ExprKind::Scan {
            source, init, body, ..
        } => {
            desugar_expr(source);
            desugar_expr(init);
            desugar_expr(body);
        }
        ExprKind::Unfold { init, body, .. } => {
            desugar_expr(init);
            desugar_expr(body);
        }
        ExprKind::Match { scrutinee, arms } => {
            desugar_expr(scrutinee);
            for arm in arms {
                desugar_expr(&mut arm.body);
            }
        }
        // `Sugar(_)` carries `Infallible` for `Desugared` — unreachable.
        #[expect(
            clippy::uninhabited_references,
            reason = "Sugar(Infallible) — proof of unreachability"
        )]
        ExprKind::Sugar(s) => match *s {},
    }

    // Now desugar TupleMatch at this node.
    if let ExprKind::TupleMatch { scrutinees, arms } = &mut expr.kind {
        // Recurse into children first.
        for s in scrutinees.iter_mut() {
            desugar_expr(s);
        }
        for arm in arms.iter_mut() {
            if let Some(patterns) = &mut arm.patterns {
                for p in patterns {
                    desugar_expr(p);
                }
            }
            desugar_expr(&mut arm.body);
        }

        let arms = arms.clone();
        let span = expr.span;

        expr.kind = desugar_tuple_match(scrutinees, arms, span);
    }
}

/// Build a nested `if` / `BinOp(Eq)` chain from tuple match scrutinees and arms.
///
/// For `match (a, b) { (X, Y) => e1, (P, Q) => e2, _ => e3 }`:
/// ```text
/// if a == X && b == Y { e1 }
/// else if a == P && b == Q { e2 }
/// else { e3 }
/// ```
fn desugar_tuple_match(
    scrutinees: &NonEmpty<Expr<crate::syntax::phase::Desugared>>,
    arms: NonEmpty<TupleMatchArm<crate::syntax::phase::Desugared>>,
    span: Span,
) -> ExprKind<crate::syntax::phase::Desugared> {
    let false_expr = Expr::new(ExprKind::Bool(false), span);

    // Build the chain from last arm to first.
    let mut result: Option<Expr<crate::syntax::phase::Desugared>> = None;

    for arm in arms.into_iter().rev() {
        match arm.patterns {
            None => {
                // Wildcard arm becomes the else branch.
                result = Some(arm.body);
            }
            Some(patterns) => {
                // Build `scrutinee[0] == pattern[0] && scrutinee[1] == pattern[1] && ...`
                let condition = build_conjunction(scrutinees, &patterns, arm.span);
                let else_branch = result.unwrap_or_else(|| false_expr.clone());
                result = Some(Expr::new(
                    ExprKind::If {
                        condition: Box::new(condition),
                        then_branch: Box::new(arm.body),
                        else_branch: Box::new(else_branch),
                    },
                    arm.span,
                ));
            }
        }
    }

    result.unwrap_or(false_expr).kind
}

/// Build `a == X && b == Y && ...` from parallel scrutinee/pattern slices.
///
/// # Panics
///
/// Panics if `scrutinees` is empty (parser guarantees at least one).
#[expect(
    clippy::unreachable,
    reason = "invariant: parser guarantees arity >= 1"
)]
fn build_conjunction(
    scrutinees: &NonEmpty<Expr<crate::syntax::phase::Desugared>>,
    patterns: &NonEmpty<Expr<crate::syntax::phase::Desugared>>,
    span: Span,
) -> Expr<crate::syntax::phase::Desugared> {
    scrutinees
        .iter()
        .zip(patterns.iter())
        .map(|(s, p)| {
            Expr::new(
                ExprKind::BinOp {
                    op: BinOp::Eq,
                    lhs: Box::new(s.clone()),
                    rhs: Box::new(p.clone()),
                },
                span,
            )
        })
        .reduce(|acc, eq| {
            Expr::new(
                ExprKind::BinOp {
                    op: BinOp::And,
                    lhs: Box::new(acc),
                    rhs: Box::new(eq),
                },
                span,
            )
        })
        // The parser guarantees at least one scrutinee.
        .unwrap_or_else(|| unreachable!("tuple match must have at least one scrutinee"))
}
