use crate::error::{err, Error};
use crate::token::{Kind, Token};
use crate::MAX_DEPTH;

// ─────────────────────────────────────────────────────────────── parse ──

#[derive(Debug, Clone)]
pub enum Node {
    Leaf(Token),
    Group(Group),
}

/// A call (`SUM(...)`), a bare paren group (`(a + b)`), or an array (`{1, 2}`).
#[derive(Debug, Clone)]
pub struct Group {
    /// Function name for a call; `None` for a bare paren group or array.
    pub head: Option<Token>,
    pub open: Token,
    /// One entry per argument. An empty entry is a blank argument and is
    /// reproduced as such — `IF(x,,y)` never becomes `IF(x,"",y)`.
    pub args: Vec<Vec<Node>>,
    /// Separator tokens between arguments, so array row breaks (`;`) survive.
    pub seps: Vec<Token>,
    pub close: Token,
    /// True when the head is a `LET`/`LAMBDA`-bound name at this position in
    /// evaluation order (see [`mark_bound_heads`]): the call resolves to the
    /// passed value, not any builtin it shadows, so name-driven treatment —
    /// uppercase rewriting, pair layout — must not apply. Set by a pass over
    /// the parsed tree, not by the parser itself.
    pub(crate) bound_head: bool,
}

impl Group {
    /// True for a call written with no arguments at all, e.g. `NOW()`.
    pub(crate) fn is_empty_call(&self) -> bool {
        self.seps.is_empty() && self.args.len() == 1 && self.args[0].is_empty()
    }

    pub(crate) fn is_array(&self) -> bool {
        self.open.text == "{"
    }

    pub(crate) fn name_upper(&self) -> String {
        self.head
            .as_ref()
            .map(|h| h.text.to_ascii_uppercase())
            .unwrap_or_default()
    }
}

/// Parse a token stream into a nesting tree.
///
/// # Errors
///
/// Returns an error for unbalanced or mismatched brackets, a stray closing
/// bracket, or nesting deeper than `MAX_DEPTH` levels.
pub fn parse(toks: &[Token]) -> Result<Vec<Node>, Error> {
    let (items, next) = parse_items(toks, 0, 0)?;
    if next < toks.len() {
        let t = &toks[next];
        return err(format!("unexpected {:?}", t.text), t.pos);
    }
    Ok(items)
}

pub(crate) fn parse_items(
    toks: &[Token],
    mut i: usize,
    depth: usize,
) -> Result<(Vec<Node>, usize), Error> {
    let mut out = Vec::new();
    while i < toks.len() {
        match toks[i].kind {
            Kind::Sep | Kind::Close => break,
            Kind::Open => {
                let (g, ni) = parse_group(toks, i, None, depth)?;
                out.push(Node::Group(g));
                i = ni;
            }
            Kind::Atom
                if i + 1 < toks.len()
                    && toks[i + 1].kind == Kind::Open
                    && toks[i + 1].text == "(" =>
            {
                let (g, ni) = parse_group(toks, i + 1, Some(toks[i].clone()), depth)?;
                out.push(Node::Group(g));
                i = ni;
            }
            _ => {
                out.push(Node::Leaf(toks[i].clone()));
                i += 1;
            }
        }
    }
    Ok((out, i))
}

pub(crate) fn parse_group(
    toks: &[Token],
    open_i: usize,
    head: Option<Token>,
    depth: usize,
) -> Result<(Group, usize), Error> {
    let open = toks[open_i].clone();
    if depth >= MAX_DEPTH {
        return err(format!("nesting exceeds {MAX_DEPTH} levels"), open.pos);
    }
    let closer = if open.text == "(" { ")" } else { "}" };
    let mut args = Vec::new();
    let mut seps = Vec::new();
    let mut i = open_i + 1;

    loop {
        let (items, ni) = parse_items(toks, i, depth + 1)?;
        args.push(items);
        i = ni;

        if i >= toks.len() {
            return err(
                format!("unbalanced {:?} — missing {closer:?}", open.text),
                open.pos,
            );
        }
        match toks[i].kind {
            Kind::Sep => {
                seps.push(toks[i].clone());
                i += 1;
            }
            Kind::Close => {
                if toks[i].text != closer {
                    return err(
                        format!("mismatched {:?} — expected {closer:?}", toks[i].text),
                        toks[i].pos,
                    );
                }
                return Ok((
                    Group {
                        head,
                        open,
                        args,
                        seps,
                        close: toks[i].clone(),
                        bound_head: false,
                    },
                    i + 1,
                ));
            }
            _ => unreachable!("parse_items stops only at Sep or Close"),
        }
    }
}

pub(crate) fn first_token(items: &[Node]) -> Option<&Token> {
    match items.first()? {
        Node::Leaf(t) => Some(t),
        Node::Group(g) => Some(g.head.as_ref().unwrap_or(&g.open)),
    }
}

/// Rewrite `;` argument separators to `,` inside calls and paren groups.
///
/// Sheets itself accepts `;` between arguments in the dot locale and
/// rewrites it to `,` the moment the formula is entered; gsfmt mirrors
/// that. This is the one sanctioned exception to "token bytes are copied
/// verbatim". Array row separators are semantic — `{1;2}` is a column,
/// `{1,2}` a row — so array groups keep theirs, and under
/// [`crate::Decimal::Comma`] nothing is touched because `;` is the only argument
/// separator there.
pub(crate) fn normalize_separators(items: &mut [Node]) {
    for node in items {
        if let Node::Group(g) = node {
            if !g.is_array() {
                for sep in &mut g.seps {
                    if sep.text == ";" {
                        sep.text = ",".into();
                    }
                }
            }
            for arg in &mut g.args {
                normalize_separators(arg);
            }
        }
    }
}

/// Rewrite call heads to their uppercase form, mirroring what the Sheets
/// editor does to builtin function names the moment a formula is entered.
/// The second sanctioned exception to "token bytes are copied verbatim",
/// and opt-in ([`crate::Options::uppercase_functions`]).
///
/// Bound-name matching uses Unicode case *folding* (see [`fold_name`]),
/// not ASCII or plain uppercase conversion: Sheets names are
/// case-insensitive beyond ASCII too, and a weaker fold would treat a
/// bound call site as unbound and mangle it — `näme` under an ASCII
/// fold, or `k` bound as the Kelvin sign `K` under uppercase conversion.
///
/// Names bound by a `LET` or `LAMBDA` are user identifiers, not builtins:
/// rewriting an invocation `myTax(2)` while its binding site stays an
/// argument token would tear the two apart visually, so bound names keep
/// their authored case. Bindings follow Sheets' evaluation-order scope —
/// computed once by [`mark_bound_heads`], which must have run first: a
/// `LET` name is visible only to *subsequent* value expressions and the
/// final expression — `sum` in `LET(sum, sum(A1:A2), sum)` is a builtin
/// call in the value position, so it uppercases — and a `LAMBDA`
/// parameter is visible only in its body. Outside the binder the same
/// name is a builtin again. Named functions (defined at the spreadsheet
/// level) are indistinguishable from builtins here and are uppercased too;
/// the Sheets named-function editor already forces their names uppercase.
pub(crate) fn uppercase_function_heads(items: &mut [Node]) {
    for node in items {
        let Node::Group(g) = node else { continue };
        if !g.bound_head {
            if let Some(h) = &mut g.head {
                h.text = h.text.to_uppercase();
            }
        }
        for arg in &mut g.args {
            uppercase_function_heads(arg);
        }
    }
}

/// Case-insensitive matching key for a name.
///
/// `str::to_uppercase` alone is case *conversion*, not case *folding*:
/// U+212A KELVIN SIGN uppercases to itself yet folds to `k`, and `ẞ`
/// lowercases to `ß` while `ß` uppercases to `SS` — single round-trips
/// key those pairs differently. Lowercase → uppercase → lowercase routes
/// every variant through the expanded uppercase form first, keying all
/// the known conversion/folding divergences identically (`ẞ`/`ß` → `ss`,
/// Kelvin → `k`, final sigma → `σ`, Cherokee). Still an approximation of
/// UCD full case folding — exact folding needs the Unicode tables this
/// dependency-free crate deliberately avoids (same stance as [`crate::layout::width::cols`]) —
/// but both sides of every comparison go through this same key, so any
/// residual divergence is at least consistent.
pub(crate) fn fold_name(s: &str) -> String {
    s.to_lowercase().to_uppercase().to_lowercase()
}

/// The binding-site name of `arg`, case-folded via [`fold_name`]. A
/// binding site is a single atom when the formula is well-formed;
/// anything else yields nothing rather than guessing.
pub(crate) fn binding_name(arg: &[Node]) -> Option<String> {
    match arg {
        [Node::Leaf(t)] if t.kind == Kind::Atom => Some(fold_name(&t.text)),
        _ => None,
    }
}

/// Walk `items` setting [`Group::bound_head`], with `scope` as the stack
/// of `LET`/`LAMBDA` names bound at this point in evaluation order.
///
/// `LET(n1, v1, n2, v2, …, body)` binds each `nᵢ` starting *after* `vᵢ`
/// (a name is not visible in its own value expression);
/// `LAMBDA(p1, …, pn, body)` binds all but the last argument, visible in
/// the body. Either way the bindings pop when the binder's group ends —
/// they never leak to siblings or ancestors.
///
/// A call whose head is itself a bound name resolves to the passed value,
/// not the builtin it shadows — Google documents `LAMBDA` placeholders as
/// taking precedence over builtin names — so a bound `let(…)`/`lambda(…)`
/// is an ordinary user-function call: it neither binds its arguments nor
/// receives any other name-driven treatment (uppercasing, pair layout).
pub(crate) fn mark_bound_heads(items: &mut [Node], scope: &mut Vec<String>) {
    for node in items {
        let Node::Group(g) = node else { continue };
        g.bound_head = g
            .head
            .as_ref()
            .is_some_and(|h| scope.contains(&fold_name(&h.text)));
        let n = g.args.len();
        let depth = scope.len();
        let binder = if g.bound_head {
            String::new()
        } else {
            g.name_upper()
        };
        match binder.as_str() {
            "LET" if n >= 3 => {
                let mut pending = None;
                for (i, arg) in g.args.iter_mut().enumerate() {
                    mark_bound_heads(arg, scope);
                    if i + 1 < n && i % 2 == 0 {
                        pending = binding_name(arg);
                    } else if let Some(name) = pending.take() {
                        scope.push(name);
                    }
                }
            }
            "LAMBDA" if n >= 2 => {
                for (i, arg) in g.args.iter_mut().enumerate() {
                    mark_bound_heads(arg, scope);
                    if i + 1 < n {
                        if let Some(name) = binding_name(arg) {
                            scope.push(name);
                        }
                    }
                }
            }
            _ => {
                for arg in &mut g.args {
                    mark_bound_heads(arg, scope);
                }
            }
        }
        scope.truncate(depth);
    }
}
