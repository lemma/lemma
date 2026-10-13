//! The one printer for Lemma expression syntax.
//!
//! Source formatting ([`crate::parsing::ast::Expression`]) and explanations
//! (normal-form cells) both print through [`Syntax`], so an expression reads the
//! same in `lemma format` output and in an explanation tree. Each tree type maps
//! its nodes onto [`SyntaxView`]; precedence, parentheses and bare-vs-call form
//! are decided here only.
//!
//! Printing never recurses into children: a node expands to a list of pieces, where
//! child nodes are pieces too, and [`render`] drives them with an explicit stack.

use crate::parsing::ast::{
    ArithmeticComputation, CalendarPeriodUnit, ComparisonComputation, DateCalendarKind,
    DateRelativeKind, RowCondition,
};
use std::fmt::{self, Write};

/// A node of an expression tree that can be printed as Lemma syntax.
pub(crate) trait SyntaxNode<'a>: Copy {
    /// Operand that prints as itself: literal, reference, path, `now`, `veto`, type.
    type Atom: fmt::Display + Clone;

    fn view(self) -> SyntaxView<'a, Self>;
}

/// Printable shape of one node.
pub(crate) enum SyntaxView<'a, N: SyntaxNode<'a>> {
    Atom {
        atom: N::Atom,
        starts_with_sign: bool,
    },
    /// `a and b and c`: operands joined by `and`.
    Conjunction(Vec<N>),
    /// Chain `a op1 b op2 c` on one precedence level. `ops[i]` joins
    /// `operands[i]` and `operands[i + 1]`; `level` gives the precedence and associativity.
    Chain {
        level: &'a ArithmeticComputation,
        operands: Vec<N>,
        ops: Vec<&'a ArithmeticComputation>,
    },
    Comparison(N, &'a ComparisonComputation, N),
    UnitConversion(N, &'a dyn fmt::Display),
    Not(N),
    IsVeto(N),
    IsNotVeto(N),
    /// `floor x`, `lower r`: keyword taking one operand.
    Prefix(&'a dyn fmt::Display, N),
    DateRelative(&'a DateRelativeKind, N),
    DateCalendar(&'a DateCalendarKind, &'a CalendarPeriodUnit, N),
    RangeLiteral(N, N),
    PastFutureRange(&'a DateRelativeKind, N),
    RangeContainment(N, N),
    ListCommand {
        list: N,
        command: CommandView<'a, N>,
    },
    Member {
        base: N,
        path: N::Atom,
        parenthesize_base: bool,
    },
    Negate(N),
    Reciprocal(N),
    /// Default result first, then each `unless` arm.
    Piecewise(Vec<(N, N)>),
}

/// Postfix `->` command.
pub(crate) enum CommandView<'a, N: SyntaxNode<'a>> {
    Find(Vec<N>),
    First,
    Last,
    Count,
    Reduce(&'static str, Option<N>),
    Predicate(&'static str, PredicateView<'a, N>),
    Sort {
        descending: bool,
        argument: Option<N>,
    },
    Top(N),
    With(Vec<(Option<N::Atom>, N)>),
}

/// `where` / `any` / `all` argument.
pub(crate) enum PredicateView<'a, N> {
    Value { op: &'a RowCondition, right: N },
    Instance(N),
}

/// Prints `N` as Lemma syntax. `{:#}` prints the tail in call form.
pub(crate) struct Syntax<N>(pub N);

/// Prints a command as Lemma syntax. `{:#}` prints it in call form.
pub(crate) struct CommandSyntax<'v, 'a, N: SyntaxNode<'a>>(pub &'v CommandView<'a, N>);

/// Operand position under a parent operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperandSide {
    Left,
    Right,
}

/// Associativity of a binary (or n-ary chain) operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Associativity {
    Left,
    Right,
}

/// Precedence for an arithmetic operator. Must match [`expression_precedence`].
pub fn arithmetic_precedence(op: &ArithmeticComputation) -> u8 {
    match op {
        ArithmeticComputation::Add | ArithmeticComputation::Subtract => 5,
        ArithmeticComputation::Multiply
        | ArithmeticComputation::Divide
        | ArithmeticComputation::Modulo => 6,
        ArithmeticComputation::Power => 7,
    }
}

pub fn arithmetic_associativity(op: &ArithmeticComputation) -> Associativity {
    match op {
        ArithmeticComputation::Power => Associativity::Right,
        ArithmeticComputation::Add
        | ArithmeticComputation::Subtract
        | ArithmeticComputation::Multiply
        | ArithmeticComputation::Divide
        | ArithmeticComputation::Modulo => Associativity::Left,
    }
}

/// Whether a child expression must be wrapped in parentheses when printed under a parent.
///
/// - `parent_assoc == None`: unary / prefix parent — wrap only when `child_prec < parent_prec`.
/// - `Some(Left)`: wrap looser children, and same-prec **right** children.
/// - `Some(Right)`: wrap looser children, and same-prec **left** children.
fn operand_needs_parentheses(
    child_prec: u8,
    parent_prec: u8,
    side: OperandSide,
    parent_assoc: Option<Associativity>,
) -> bool {
    if child_prec < parent_prec {
        return true;
    }
    if child_prec > parent_prec {
        return false;
    }
    match parent_assoc {
        None => false,
        Some(Associativity::Left) => matches!(side, OperandSide::Right),
        Some(Associativity::Right) => matches!(side, OperandSide::Left),
    }
}

/// Precedence level of a node. Higher binds tighter.
///
/// `...` binds tighter than prefix operators, so `lower 5...10` is `lower` of the
/// range and prints without parentheses. A prefix that is itself a range endpoint
/// prints in parentheses: `(lower start)...(upper start)`.
/// Prefix operators bind tighter than `as` and arithmetic.
/// Parser climb in [`crate::parsing::parser::Parser`] must match this table.
fn expression_precedence<'a, N: SyntaxNode<'a>>(node: N) -> u8 {
    view_precedence(&node.view())
}

fn view_precedence<'a, N: SyntaxNode<'a>>(view: &SyntaxView<'a, N>) -> u8 {
    match view {
        SyntaxView::Conjunction(_) => 2,
        SyntaxView::Not(_) | SyntaxView::IsNotVeto(_) | SyntaxView::Negate(_) => 3,
        SyntaxView::Comparison(..)
        | SyntaxView::IsVeto(_)
        | SyntaxView::RangeContainment(..)
        | SyntaxView::DateRelative(..)
        | SyntaxView::DateCalendar(..) => 4,
        SyntaxView::Chain { level, .. } => arithmetic_precedence(level),
        SyntaxView::Reciprocal(_) => arithmetic_precedence(&ArithmeticComputation::Multiply),
        SyntaxView::UnitConversion(..) => 8,
        SyntaxView::Prefix(..) => 9,
        SyntaxView::RangeLiteral(..)
        | SyntaxView::PastFutureRange(..)
        | SyntaxView::Atom { .. }
        | SyntaxView::Member { .. }
        | SyntaxView::Piecewise(_) => 10,
        SyntaxView::ListCommand { .. } => 11,
    }
}

/// One operand that may stand as a bare command argument (`sum bonus`, `top 3`).
/// A leading sign is excluded: after a command, `-` is the binary operator.
fn is_bare_argument<'a, N: SyntaxNode<'a>>(node: N) -> bool {
    is_comparison_operand(node) && !starts_with_sign(node)
}

fn starts_with_sign<'a, N: SyntaxNode<'a>>(mut node: N) -> bool {
    loop {
        match node.view() {
            SyntaxView::Atom {
                starts_with_sign, ..
            } => return starts_with_sign,
            SyntaxView::RangeLiteral(left, _) => node = left,
            _ => return false,
        }
    }
}

/// One operand on either side of a bare condition's comparison.
fn is_comparison_operand<'a, N: SyntaxNode<'a>>(node: N) -> bool {
    expression_precedence(node) == 10
}

/// The list before `->` stands bare only as one primary; `->` binds tighter than a range's `...`.
fn is_command_list_operand<'a, N: SyntaxNode<'a>>(list: N) -> bool {
    match list.view() {
        SyntaxView::ListCommand { .. } => true,
        SyntaxView::RangeLiteral(..) | SyntaxView::PastFutureRange(..) => false,
        _ => is_bare_argument(list),
    }
}

/// Operand a prefix operator (`floor`, `lower`, `not`) takes without parentheses:
/// one operand, a `->` chain, or another bare prefix operator (`floor abs x`).
fn is_prefix_bare_operand<'a, N: SyntaxNode<'a>>(mut operand: N) -> bool {
    loop {
        match operand.view() {
            SyntaxView::ListCommand { .. } => return true,
            SyntaxView::Prefix(_, inner) => operand = inner,
            _ => return is_bare_argument(operand),
        }
    }
}

fn is_bare_predicate<'a, N: SyntaxNode<'a>>(form: &PredicateView<'a, N>) -> bool {
    match form {
        PredicateView::Value { right, .. } => is_comparison_operand(*right),
        PredicateView::Instance(expr) => match expr.view() {
            SyntaxView::Comparison(left, _, right) | SyntaxView::RangeContainment(left, right) => {
                is_bare_argument(left) && is_comparison_operand(right)
            }
            _ => is_bare_argument(*expr),
        },
    }
}

/// True when the command prints as a bare argument, so nothing but `->`, `then`,
/// `unless`, `)`, `,` or the end may follow it.
fn command_ends_with_bare_argument<'a, N: SyntaxNode<'a>>(command: &CommandView<'a, N>) -> bool {
    match command {
        CommandView::Find(keys) => keys.iter().all(|key| is_bare_argument(*key)),
        CommandView::Reduce(_, argument) => argument.is_some_and(is_bare_argument),
        CommandView::Predicate(_, form) => is_bare_predicate(form),
        CommandView::Top(n) => is_bare_argument(*n),
        CommandView::With(_) => true,
        CommandView::First | CommandView::Last | CommandView::Count | CommandView::Sort { .. } => {
            false
        }
    }
}

/// Outcome of one link of the right-spine walk in [`ends_with_bare_argument`].
enum TailStep<N> {
    Decided(bool),
    Continue(N),
}

/// True when the printed expression ends in a bare argument, so an operator after it
/// would be a parse Error. Such an operand prints its tail in call form (`{:#}`):
/// `staff -> sum(bonus) * 2`, `floor(x) + 1`.
fn ends_with_bare_argument<'a, N: SyntaxNode<'a>>(mut node: N) -> bool {
    loop {
        let view = node.view();
        let my_prec = view_precedence(&view);
        let right_spine = |right: N, parent_prec: u8, assoc: Option<Associativity>| {
            if operand_needs_parentheses(
                expression_precedence(right),
                parent_prec,
                OperandSide::Right,
                assoc,
            ) {
                TailStep::Decided(false)
            } else {
                TailStep::Continue(right)
            }
        };
        let step = match view {
            SyntaxView::ListCommand { command, .. } => {
                TailStep::Decided(command_ends_with_bare_argument(&command))
            }
            SyntaxView::Chain {
                level, operands, ..
            } => right_spine(
                *operands
                    .last()
                    .expect("BUG: n-ary chain must have operands"),
                my_prec,
                Some(arithmetic_associativity(level)),
            ),
            SyntaxView::Conjunction(operands) => right_spine(
                *operands
                    .last()
                    .expect("BUG: conjunction must have operands"),
                my_prec,
                Some(Associativity::Left),
            ),
            SyntaxView::Comparison(_, _, right)
            | SyntaxView::RangeContainment(_, right)
            | SyntaxView::RangeLiteral(_, right)
            | SyntaxView::PastFutureRange(_, right) => right_spine(right, my_prec, None),
            SyntaxView::Negate(right) => right_spine(
                right,
                arithmetic_precedence(&ArithmeticComputation::Multiply),
                Some(Associativity::Left),
            ),
            SyntaxView::Reciprocal(right) => right_spine(right, my_prec, Some(Associativity::Left)),
            SyntaxView::Prefix(_, operand) | SyntaxView::Not(operand) => {
                TailStep::Decided(is_prefix_bare_operand(operand))
            }
            SyntaxView::Atom { .. }
            | SyntaxView::UnitConversion(..)
            | SyntaxView::IsVeto(_)
            | SyntaxView::IsNotVeto(_)
            | SyntaxView::DateRelative(..)
            | SyntaxView::DateCalendar(..)
            | SyntaxView::Member { .. }
            | SyntaxView::Piecewise(_) => TailStep::Decided(false),
        };
        match step {
            TailStep::Decided(answer) => return answer,
            TailStep::Continue(next) => node = next,
        }
    }
}

/// One printed fragment. A `Node` expands into its own fragments when reached, so
/// printing a tree never recurses on its children.
enum Piece<'a, N: SyntaxNode<'a>> {
    Text(&'static str),
    Owned(String),
    Shown(&'a dyn fmt::Display),
    Atom(N::Atom),
    Node { node: N, alternate: bool },
}

/// `line_break` applies to the first node expanded (the root) only: its top-level
/// `and` / arithmetic operators start a new line at that indent.
fn render<'a, N: SyntaxNode<'a>>(
    out: &mut impl Write,
    pieces: Vec<Piece<'a, N>>,
    mut line_break: Option<&str>,
) -> fmt::Result {
    let mut stack: Vec<Piece<'a, N>> = pieces.into_iter().rev().collect();
    while let Some(piece) = stack.pop() {
        match piece {
            Piece::Text(text) => out.write_str(text)?,
            Piece::Owned(text) => out.write_str(&text)?,
            Piece::Shown(shown) => write!(out, "{shown}")?,
            Piece::Atom(atom) => write!(out, "{atom}")?,
            Piece::Node { node, alternate } => {
                let pieces = node_pieces(node, alternate, line_break.take());
                stack.extend(pieces.into_iter().rev());
            }
        }
    }
    Ok(())
}

/// Space before a chain operator, or a new line at `indent` when the chain is broken.
fn separator<'a, N: SyntaxNode<'a>>(line_break: Option<&str>) -> Piece<'a, N> {
    match line_break {
        Some(indent) => Piece::Owned(format!("\n{indent}")),
        None => Piece::Text(" "),
    }
}

fn push_parenthesized<'a, N: SyntaxNode<'a>>(out: &mut Vec<Piece<'a, N>>, node: N) {
    out.push(Piece::Text("("));
    out.push(Piece::Node {
        node,
        alternate: false,
    });
    out.push(Piece::Text(")"));
}

/// Operand of a binary operator. Precedence decides parentheses.
///
/// An operand ending in a bare argument uses call form on either side, so both
/// operands read alike: `sqrt(4) * sqrt(9)`, `2 * lines -> sum(qty)`. On the right,
/// `not` keeps its bare form: `a and not b`. On the left it cannot: `not(a) and b`.
fn binary_operand<'a, N: SyntaxNode<'a>>(
    out: &mut Vec<Piece<'a, N>>,
    child: N,
    parent_prec: u8,
    side: OperandSide,
    parent_assoc: Option<Associativity>,
) {
    if operand_needs_parentheses(
        expression_precedence(child),
        parent_prec,
        side,
        parent_assoc,
    ) {
        push_parenthesized(out, child);
        return;
    }
    let call_form = match side {
        OperandSide::Left => ends_with_bare_argument(child),
        OperandSide::Right => {
            !matches!(child.view(), SyntaxView::Not(_)) && ends_with_bare_argument(child)
        }
    };
    out.push(Piece::Node {
        node: child,
        alternate: call_form,
    });
}

/// Right operand of a unary operator (`-x`, `1/x`, `past x`). It prints its tail in
/// call form (`{:#}`) exactly when its parent does.
fn unary_operand<'a, N: SyntaxNode<'a>>(
    out: &mut Vec<Piece<'a, N>>,
    child: N,
    parent_prec: u8,
    parent_assoc: Option<Associativity>,
    alternate: bool,
) {
    if operand_needs_parentheses(
        expression_precedence(child),
        parent_prec,
        OperandSide::Right,
        parent_assoc,
    ) {
        push_parenthesized(out, child);
    } else {
        out.push(Piece::Node {
            node: child,
            alternate,
        });
    }
}

/// `{child:#}` when `child` ends in a bare argument and something follows it.
fn call_tail<'a, N: SyntaxNode<'a>>(out: &mut Vec<Piece<'a, N>>, child: N) {
    out.push(Piece::Node {
        node: child,
        alternate: ends_with_bare_argument(child),
    });
}

/// `kw operand` when the operand stands bare and nothing follows; `kw(operand)` otherwise.
fn prefix_operator<'a, N: SyntaxNode<'a>>(
    out: &mut Vec<Piece<'a, N>>,
    keyword: Piece<'a, N>,
    operand: N,
    alternate: bool,
) {
    out.push(keyword);
    if !alternate && is_prefix_bare_operand(operand) {
        out.push(Piece::Text(" "));
        out.push(Piece::Node {
            node: operand,
            alternate: false,
        });
    } else {
        push_parenthesized(out, operand);
    }
}

fn comparison_pieces<'a, N: SyntaxNode<'a>>(
    left: N,
    op: Piece<'a, N>,
    right: N,
) -> Vec<Piece<'a, N>> {
    let comparison_prec = 4;
    let mut out = Vec::new();
    binary_operand(&mut out, left, comparison_prec, OperandSide::Left, None);
    out.push(Piece::Text(" "));
    out.push(op);
    out.push(Piece::Text(" "));
    binary_operand(&mut out, right, comparison_prec, OperandSide::Right, None);
    out
}

/// `left op right` printed with the shared operand rules.
pub(crate) fn display_comparison<'a, N: SyntaxNode<'a>>(
    left: N,
    op: &ComparisonComputation,
    right: N,
) -> String {
    let mut text = String::new();
    render(
        &mut text,
        comparison_pieces(left, Piece::Owned(op.to_string()), right),
        None,
    )
    .expect("BUG: writing to a String cannot fail");
    text
}

/// `node` with each top-level `and` / arithmetic operator starting a new line at `indent`.
pub(crate) fn display_broken<'a, N: SyntaxNode<'a>>(node: N, indent: &str) -> String {
    let mut text = String::new();
    render(
        &mut text,
        vec![Piece::Node {
            node,
            alternate: false,
        }],
        Some(indent),
    )
    .expect("BUG: writing to a String cannot fail");
    text
}

fn node_pieces<'a, N: SyntaxNode<'a>>(
    node: N,
    alternate: bool,
    line_break: Option<&str>,
) -> Vec<Piece<'a, N>> {
    let view = node.view();
    let my_prec = view_precedence(&view);
    let mut out = Vec::new();
    match view {
        SyntaxView::Atom { atom, .. } => out.push(Piece::Atom(atom)),
        SyntaxView::Chain {
            level,
            operands,
            ops,
        } => {
            let assoc = Some(arithmetic_associativity(level));
            let mut ops = ops.into_iter();
            for (i, operand) in operands.into_iter().enumerate() {
                let side = if i == 0 {
                    OperandSide::Left
                } else {
                    let op = ops
                        .next()
                        .expect("BUG: chain has one operator per operand after the first");
                    out.push(separator(line_break));
                    out.push(Piece::Shown(op));
                    out.push(Piece::Text(" "));
                    OperandSide::Right
                };
                binary_operand(&mut out, operand, my_prec, side, assoc);
            }
        }
        SyntaxView::Comparison(left, op, right) => {
            out.extend(comparison_pieces(left, Piece::Shown(op), right));
        }
        SyntaxView::UnitConversion(value, target) => {
            binary_operand(&mut out, value, my_prec, OperandSide::Left, None);
            out.push(Piece::Text(" as "));
            out.push(Piece::Shown(target));
        }
        SyntaxView::IsNotVeto(operand) => {
            binary_operand(&mut out, operand, my_prec, OperandSide::Left, None);
            out.push(Piece::Text(" is not veto"));
        }
        SyntaxView::Not(operand) => {
            prefix_operator(&mut out, Piece::Text("not"), operand, alternate);
        }
        SyntaxView::IsVeto(operand) => {
            binary_operand(&mut out, operand, my_prec, OperandSide::Left, None);
            out.push(Piece::Text(" is veto"));
        }
        SyntaxView::Conjunction(operands) => {
            let assoc = Some(Associativity::Left);
            for (i, operand) in operands.into_iter().enumerate() {
                let side = if i == 0 {
                    OperandSide::Left
                } else {
                    out.push(separator(line_break));
                    out.push(Piece::Text("and "));
                    OperandSide::Right
                };
                binary_operand(&mut out, operand, my_prec, side, assoc);
            }
        }
        SyntaxView::Prefix(keyword, operand) => {
            prefix_operator(&mut out, Piece::Shown(keyword), operand, alternate);
        }
        SyntaxView::DateRelative(kind, date) => {
            call_tail(&mut out, date);
            out.push(Piece::Text(" "));
            out.push(Piece::Shown(kind));
        }
        SyntaxView::DateCalendar(kind, unit, date) => {
            call_tail(&mut out, date);
            out.push(Piece::Text(" "));
            out.push(Piece::Shown(kind));
            out.push(Piece::Text(" "));
            out.push(Piece::Shown(unit));
        }
        SyntaxView::RangeLiteral(left, right) => {
            binary_operand(&mut out, left, my_prec, OperandSide::Left, None);
            out.push(Piece::Text("..."));
            binary_operand(&mut out, right, my_prec, OperandSide::Right, None);
        }
        SyntaxView::PastFutureRange(kind, offset) => {
            match kind {
                DateRelativeKind::InPast => out.push(Piece::Text("past ")),
                DateRelativeKind::InFuture => out.push(Piece::Text("future ")),
            }
            unary_operand(&mut out, offset, my_prec, None, alternate);
        }
        SyntaxView::RangeContainment(value, range) => {
            binary_operand(&mut out, value, my_prec, OperandSide::Left, None);
            out.push(Piece::Text(" in "));
            binary_operand(&mut out, range, my_prec, OperandSide::Right, None);
        }
        SyntaxView::ListCommand { list, command } => {
            if is_command_list_operand(list) {
                out.push(Piece::Node {
                    node: list,
                    alternate: false,
                });
            } else {
                push_parenthesized(&mut out, list);
            }
            out.push(Piece::Text(" -> "));
            out.extend(command_pieces(&command, alternate));
        }
        SyntaxView::Member {
            base,
            path,
            parenthesize_base,
        } => {
            if parenthesize_base {
                out.push(Piece::Text("("));
                out.push(Piece::Node {
                    node: base,
                    alternate: false,
                });
                out.push(Piece::Text(")."));
            } else {
                out.push(Piece::Node {
                    node: base,
                    alternate: false,
                });
                out.push(Piece::Text("."));
            }
            out.push(Piece::Atom(path));
        }
        SyntaxView::Negate(operand) => {
            out.push(Piece::Text("-"));
            unary_operand(
                &mut out,
                operand,
                arithmetic_precedence(&ArithmeticComputation::Multiply),
                Some(Associativity::Left),
                alternate,
            );
        }
        SyntaxView::Reciprocal(operand) => {
            out.push(Piece::Text("1/"));
            unary_operand(
                &mut out,
                operand,
                my_prec,
                Some(Associativity::Left),
                alternate,
            );
        }
        SyntaxView::Piecewise(arms) => {
            let mut arms = arms.into_iter();
            let (_, default) = arms.next().expect("BUG: piecewise must have a default arm");
            out.push(Piece::Node {
                node: default,
                alternate: false,
            });
            for (condition, result) in arms {
                out.push(Piece::Text(" unless "));
                out.push(Piece::Node {
                    node: condition,
                    alternate: false,
                });
                out.push(Piece::Text(": "));
                out.push(Piece::Node {
                    node: result,
                    alternate: false,
                });
            }
        }
    }
    out
}

/// ` arg` when the argument is one operand, `(arg)` otherwise.
fn command_argument<'a, N: SyntaxNode<'a>>(
    out: &mut Vec<Piece<'a, N>>,
    argument: N,
    alternate: bool,
) {
    if !alternate && is_bare_argument(argument) {
        out.push(Piece::Text(" "));
        out.push(Piece::Node {
            node: argument,
            alternate: false,
        });
    } else {
        push_parenthesized(out, argument);
    }
}

fn command_pieces<'a, N: SyntaxNode<'a>>(
    command: &CommandView<'a, N>,
    alternate: bool,
) -> Vec<Piece<'a, N>> {
    let mut out = Vec::new();
    match command {
        CommandView::Find(keys) => {
            let bare = !alternate && keys.iter().all(|key| is_bare_argument(*key));
            out.push(Piece::Text(if bare { "find " } else { "find(" }));
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(Piece::Text(", "));
                }
                out.push(Piece::Node {
                    node: *key,
                    alternate: false,
                });
            }
            if !bare {
                out.push(Piece::Text(")"));
            }
        }
        CommandView::First => out.push(Piece::Text("first")),
        CommandView::Last => out.push(Piece::Text("last")),
        CommandView::Count => out.push(Piece::Text("count")),
        CommandView::Reduce(name, argument) => {
            out.push(Piece::Text(name));
            if let Some(argument) = argument {
                command_argument(&mut out, *argument, alternate);
            }
        }
        CommandView::Predicate(name, form) => {
            out.push(Piece::Text(name));
            if !alternate && is_bare_predicate(form) {
                out.push(Piece::Text(" "));
                out.extend(predicate_pieces(form));
            } else {
                out.push(Piece::Text("("));
                out.extend(predicate_pieces(form));
                out.push(Piece::Text(")"));
            }
        }
        CommandView::Sort {
            descending,
            argument,
        } => {
            out.push(Piece::Text("sort"));
            if let Some(argument) = argument {
                command_argument(&mut out, *argument, alternate);
            }
            out.push(Piece::Text(if *descending { " desc" } else { " asc" }));
        }
        CommandView::Top(n) => {
            out.push(Piece::Text("top"));
            command_argument(&mut out, *n, alternate);
        }
        CommandView::With(rows) => {
            out.push(Piece::Text(if alternate { "with(" } else { "with " }));
            for (i, (label, value)) in rows.iter().enumerate() {
                if i > 0 {
                    out.push(Piece::Text(", "));
                }
                if let Some(label) = label {
                    out.push(Piece::Atom(label.clone()));
                    out.push(Piece::Text(": "));
                }
                out.push(Piece::Node {
                    node: *value,
                    alternate: false,
                });
            }
            if alternate {
                out.push(Piece::Text(")"));
            }
        }
    }
    out
}

fn predicate_pieces<'a, N: SyntaxNode<'a>>(form: &PredicateView<'a, N>) -> Vec<Piece<'a, N>> {
    match form {
        PredicateView::Value { op, right } => {
            let mut out = vec![Piece::Shown(*op)];
            if expression_precedence(*right) < arithmetic_precedence(&ArithmeticComputation::Add) {
                out.push(Piece::Text(" ("));
                out.push(Piece::Node {
                    node: *right,
                    alternate: false,
                });
                out.push(Piece::Text(")"));
            } else {
                out.push(Piece::Text(" "));
                out.push(Piece::Node {
                    node: *right,
                    alternate: false,
                });
            }
            out
        }
        PredicateView::Instance(expr) => vec![Piece::Node {
            node: *expr,
            alternate: false,
        }],
    }
}

impl<'a, N: SyntaxNode<'a>> fmt::Display for Syntax<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        render(
            f,
            vec![Piece::Node {
                node: self.0,
                alternate: f.alternate(),
            }],
            None,
        )
    }
}

impl<'a, N: SyntaxNode<'a>> fmt::Display for CommandSyntax<'_, 'a, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        render(f, command_pieces(self.0, f.alternate()), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    enum Tree {
        Name(&'static str),
        Nary(ArithmeticComputation, Vec<Tree>),
        Negate(Box<Tree>),
        Reciprocal(Box<Tree>),
        Floor(Box<Tree>),
        And(Vec<Tree>),
    }

    impl<'a> SyntaxNode<'a> for &'a Tree {
        type Atom = &'static str;

        fn view(self) -> SyntaxView<'a, Self> {
            match self {
                Tree::Name(name) => SyntaxView::Atom {
                    atom: name,
                    starts_with_sign: false,
                },
                Tree::Nary(op, operands) => SyntaxView::Chain {
                    level: op,
                    operands: operands.iter().collect(),
                    ops: (1..operands.len()).map(|_| op).collect(),
                },
                Tree::Negate(operand) => SyntaxView::Negate(operand),
                Tree::Reciprocal(operand) => SyntaxView::Reciprocal(operand),
                Tree::Floor(operand) => SyntaxView::Prefix(&"floor", operand),
                Tree::And(operands) => SyntaxView::Conjunction(operands.iter().collect()),
            }
        }
    }

    fn name(name: &'static str) -> Tree {
        Tree::Name(name)
    }

    fn sum(operands: Vec<Tree>) -> Tree {
        Tree::Nary(ArithmeticComputation::Add, operands)
    }

    fn product(operands: Vec<Tree>) -> Tree {
        Tree::Nary(ArithmeticComputation::Multiply, operands)
    }

    fn power(base: Tree, exponent: Tree) -> Tree {
        Tree::Nary(ArithmeticComputation::Power, vec![base, exponent])
    }

    fn print(tree: &Tree) -> String {
        Syntax(tree).to_string()
    }

    #[test]
    fn nary_chain_prints_operands_joined_by_operator() {
        assert_eq!(
            print(&sum(vec![name("a"), name("b"), name("c")])),
            "a + b + c"
        );
        assert_eq!(
            print(&product(vec![name("a"), name("b"), name("c")])),
            "a * b * c"
        );
    }

    #[test]
    fn nary_wraps_looser_operands() {
        let tree = product(vec![sum(vec![name("a"), name("b")]), name("c")]);
        assert_eq!(print(&tree), "(a + b) * c");
    }

    #[test]
    fn nary_keeps_tighter_operands_bare() {
        let tree = sum(vec![name("a"), product(vec![name("b"), name("c")])]);
        assert_eq!(print(&tree), "a + b * c");
    }

    #[test]
    fn nary_wraps_same_precedence_chain_after_first_operand() {
        let first = sum(vec![sum(vec![name("a"), name("b")]), name("c")]);
        assert_eq!(print(&first), "a + b + c");
        let later = sum(vec![name("a"), sum(vec![name("b"), name("c")])]);
        assert_eq!(print(&later), "a + (b + c)");
    }

    #[test]
    fn nary_operand_ending_in_bare_argument_uses_call_form() {
        let last = sum(vec![name("a"), Tree::Floor(Box::new(name("x")))]);
        assert_eq!(print(&last), "a + floor(x)");
        let first = sum(vec![Tree::Floor(Box::new(name("x"))), name("a")]);
        assert_eq!(print(&first), "floor(x) + a");
    }

    #[test]
    fn power_chain_is_right_associative() {
        let right = power(name("a"), power(name("b"), name("c")));
        assert_eq!(print(&right), "a ^ b ^ c");
        let left = power(power(name("a"), name("b")), name("c"));
        assert_eq!(print(&left), "(a ^ b) ^ c");
    }

    #[test]
    fn negate_prints_atom_bare_and_wraps_arithmetic() {
        assert_eq!(print(&Tree::Negate(Box::new(name("a")))), "-a");
        let tree = Tree::Negate(Box::new(sum(vec![name("a"), name("b")])));
        assert_eq!(print(&tree), "-(a + b)");
    }

    #[test]
    fn negate_wraps_product_and_keeps_power_bare() {
        let product_operand = Tree::Negate(Box::new(product(vec![name("a"), name("b")])));
        assert_eq!(print(&product_operand), "-(a * b)");
        let power_operand = Tree::Negate(Box::new(power(name("a"), name("b"))));
        assert_eq!(print(&power_operand), "-a ^ b");
    }

    #[test]
    fn negate_operand_of_arithmetic_is_wrapped() {
        let tree = product(vec![name("a"), Tree::Negate(Box::new(name("b")))]);
        assert_eq!(print(&tree), "a * (-b)");
    }

    #[test]
    fn reciprocal_wraps_same_precedence_operand() {
        assert_eq!(print(&Tree::Reciprocal(Box::new(name("a")))), "1/a");
        let tree = Tree::Reciprocal(Box::new(product(vec![name("a"), name("b")])));
        assert_eq!(print(&tree), "1/(a * b)");
    }

    #[test]
    fn reciprocal_keeps_power_operand_bare() {
        let tree = Tree::Reciprocal(Box::new(power(name("a"), name("b"))));
        assert_eq!(print(&tree), "1/a ^ b");
    }

    #[test]
    fn reciprocal_in_product_prints_bare_as_left_operand() {
        let tree = product(vec![Tree::Reciprocal(Box::new(name("a"))), name("b")]);
        assert_eq!(print(&tree), "1/a * b");
    }

    #[test]
    fn broken_chain_starts_each_root_operator_on_a_new_line() {
        let tree = sum(vec![
            name("a"),
            product(vec![name("b"), name("c")]),
            name("d"),
        ]);
        assert_eq!(display_broken(&tree, "  "), "a\n  + b * c\n  + d");
    }

    #[test]
    fn broken_conjunction_keeps_nested_chains_flat() {
        let tree = Tree::And(vec![
            name("a"),
            Tree::And(vec![name("b"), name("c")]),
            sum(vec![name("d"), name("e")]),
        ]);
        assert_eq!(
            display_broken(&tree, "    "),
            "a\n    and (b and c)\n    and d + e"
        );
    }
}
