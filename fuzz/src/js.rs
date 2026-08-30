//! A generator for *valid* JavaScript programs.
//!
//! WHY A GRAMMAR AND NOT BYTES. A fuzzer that mutates text finds syntax errors, and a syntax
//! error is answered identically by both engines, so it exercises nothing this crate exists to
//! measure. Every value of [`JsProgram`] renders to a syntactically valid script, so the
//! frontend always accepts it and the interesting question -- does lowering refuse it, and if
//! not do the two engines agree -- is the one actually asked.
//!
//! FOUR PROPERTIES THE GRAMMAR HAS TO GUARANTEE, because the native side has no supervisor:
//!
//! 1. **Termination.** There is no `while`, no `do`, and no user-written loop condition. The one
//!    loop form is `for (let i = 0; i < K; i++)` with `K` a literal in `0..=3`, and the induction
//!    variable is never assignable, so a body runs at most `K` times.
//! 2. **Bounded work.** Statement nesting is capped at [`MAX_STATEMENT_DEPTH`], so at worst
//!    `LOOP_BOUND ^ MAX_STATEMENT_DEPTH` body executions -- nine, today. `x = x + x` in the
//!    innermost body therefore multiplies a string by 2^9, not by 2^27. This is the property
//!    that keeps a differential run off the memory cliff; loosen `LOOP_BOUND` and the growth is
//!    exponential in the wrong exponent.
//! 3. **Determinism.** No `Date`, no `Math.random`, no `globalThis` mutation, no timers with a
//!    magnitude anyone could observe. ADR 0012 and ADR 0015 already make the realm deterministic;
//!    this keeps the generator from reaching outside it, which is what makes any difference in
//!    output a real divergence rather than ambient noise.
//! 4. **Resolvability.** Every identifier a program mentions is one it declared. ADR 0017 makes
//!    an unresolved identifier a runtime error, and a corpus of programs that all die on their
//!    first line measures the error path and nothing else.

use std::fmt::Write as _;

use arbitrary::Arbitrary;

use crate::{Leafy, Many, Nested};

/// How deep a generated expression may nest. Enforced by [`Nested`] at generation time.
type Sub = Nested<Expression, 3>;

/// Upper bound on a generated `for` loop's trip count.
const LOOP_BOUND: u32 = 3;
/// How deeply statements may nest. See property 2 above before raising it.
const MAX_STATEMENT_DEPTH: u32 = 2;
/// A backstop on expression nesting, well above the real bound.
///
/// The real bound is `Sub = Nested<Expression, 3>`, applied while the value is generated. This
/// is only here so that a change to that type cannot turn into unbounded output, and it should
/// never fire -- if it does, the two numbers have drifted apart.
const MAX_EXPRESSION_DEPTH: u32 = 12;
/// Mutable variables every program declares up front.
const VARIABLES: [&str; 4] = ["a", "b", "c", "d"];
/// Functions every program declares up front.
const FUNCTIONS: [&str; 2] = ["f0", "f1"];
/// What a name may refer to inside a function body: the outer bindings plus the parameters.
const PARAMETERS: [&str; 6] = ["a", "b", "c", "d", "p", "q"];

/// Numbers chosen for the edges rather than for the middle: the two zeroes, the two infinities,
/// NaN, the integer/double boundary, and the values where `|0` and `>>> 0` disagree with `Math`.
const NUMBERS: [&str; 14] = [
    "0",
    "-0",
    "1",
    "-1",
    "0.5",
    "-0.1",
    "2",
    "10",
    "255",
    "NaN",
    "Infinity",
    "-Infinity",
    "9007199254740993",
    "1e308",
];

/// Strings chosen for the places string semantics are decided: empty, numeric-looking (so `+`
/// and `==` have to choose), whitespace-only (`Number(" ")` is 0, not NaN), a lone surrogate,
/// and an astral pair whose length in code units is not its length in scalar values (ADR 0022).
const STRINGS: [&str; 8] = [
    r#""""#,
    r#""x""#,
    r#""0""#,
    r#""1e3""#,
    r#""  ""#,
    r#""\n""#,
    r#""😀""#,
    r#""\ud800""#,
];

#[derive(Debug, Clone, Arbitrary)]
pub enum Literal {
    Number(u8),
    Str(u8),
    Bool(bool),
    Null,
    Undefined,
}

#[derive(Debug, Clone, Copy, Arbitrary)]
pub enum UnaryOp {
    Negate,
    Plus,
    Not,
    TypeOf,
    BitwiseNot,
    Void,
}

impl UnaryOp {
    const fn spelling(self) -> &'static str {
        match self {
            Self::Negate => "-",
            Self::Plus => "+",
            Self::Not => "!",
            Self::TypeOf => "typeof ",
            Self::BitwiseNot => "~",
            Self::Void => "void ",
        }
    }
}

/// Mirrors `kiln_ir::BinaryOp`. `in` and `instanceof` are absent deliberately: both want a
/// right-hand side that is an object or a constructor, and a generator that supplies a number
/// there spends its whole budget producing the same TypeError.
#[derive(Debug, Clone, Copy, Arbitrary)]
pub enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Exponentiate,
    StrictEqual,
    StrictNotEqual,
    LooseEqual,
    LooseNotEqual,
    BitwiseAnd,
    BitwiseOr,
    BitwiseXor,
    ShiftLeft,
    ShiftRight,
    UnsignedShiftRight,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
}

impl BinaryOp {
    const fn spelling(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Subtract => "-",
            Self::Multiply => "*",
            Self::Divide => "/",
            Self::Remainder => "%",
            Self::Exponentiate => "**",
            Self::StrictEqual => "===",
            Self::StrictNotEqual => "!==",
            Self::LooseEqual => "==",
            Self::LooseNotEqual => "!=",
            Self::BitwiseAnd => "&",
            Self::BitwiseOr => "|",
            Self::BitwiseXor => "^",
            Self::ShiftLeft => "<<",
            Self::ShiftRight => ">>",
            Self::UnsignedShiftRight => ">>>",
            Self::LessThan => "<",
            Self::LessThanOrEqual => "<=",
            Self::GreaterThan => ">",
            Self::GreaterThanOrEqual => ">=",
        }
    }
}

#[derive(Debug, Clone, Copy, Arbitrary)]
pub enum LogicalOp {
    And,
    Or,
    Nullish,
}

#[derive(Debug, Clone, Arbitrary)]
pub enum Expression {
    Literal(Literal),
    /// One of the program's own `let` bindings.
    Variable(u8),
    /// The enclosing loop's induction variable, if there is one; a literal `0` if there is not,
    /// so that the choice never changes whether the program is well-formed.
    Induction,
    Unary(UnaryOp, Sub),
    Binary(BinaryOp, Sub, Sub),
    Logical(LogicalOp, Sub, Sub),
    Conditional(Sub, Sub, Sub),
    Array(Many<Sub, 3>),
    Object(Many<Sub, 3>),
    /// `.p0` / `.p1` / `.length`, which is where a hole, a getter, and an exotic length differ.
    Member(Sub, u8),
    /// `e[k]` -- the computed path, which converts the key and so takes a different route.
    Computed(Sub, Sub),
    Call(u8, Sub, Sub),
    /// `` `...${e}...` `` -- forces ToString on a value chosen by the fuzzer.
    Template(Sub),
    /// A method on one of the built-ins the language subset already has.
    Builtin(BuiltinCall, Sub),
}

/// Built-in entry points reached with exactly one argument. Kept to conversions and formatting
/// because those are where the interpreter and the C runtime hold *separate* implementations --
/// number formatting in particular is a vendored C runtime on one side (ADR 0013) and Rust on
/// the other, which is precisely a place two engines can disagree.
#[derive(Debug, Clone, Copy, Arbitrary)]
pub enum BuiltinCall {
    String,
    Number,
    Boolean,
    JsonStringify,
    MathAbs,
    MathFloor,
    MathTrunc,
    ObjectKeys,
    ArrayIsArray,
}

impl BuiltinCall {
    const fn spelling(self) -> &'static str {
        match self {
            Self::String => "String",
            Self::Number => "Number",
            Self::Boolean => "Boolean",
            Self::JsonStringify => "JSON.stringify",
            Self::MathAbs => "Math.abs",
            Self::MathFloor => "Math.floor",
            Self::MathTrunc => "Math.trunc",
            Self::ObjectKeys => "Object.keys",
            Self::ArrayIsArray => "Array.isArray",
        }
    }
}

impl Leafy for Expression {
    /// What an expression collapses to at the depth limit: something that reads state, so the
    /// leaves of a deep tree are not all the same constant.
    fn leaf(u: &mut arbitrary::Unstructured<'_>) -> arbitrary::Result<Self> {
        Ok(match u8::arbitrary(u)? % 3 {
            0 => Self::Variable(u8::arbitrary(u)?),
            1 => Self::Induction,
            _ => Self::Literal(Literal::arbitrary(u)?),
        })
    }
}

#[derive(Debug, Clone, Arbitrary)]
pub enum Statement {
    Log(Expression),
    Assign(u8, Expression),
    If(Expression, Many<Statement, 4>, Many<Statement, 4>),
    /// `for (let i = 0; i < K; i++)`, `K = bound % (LOOP_BOUND + 1)`.
    For(u8, Many<Statement, 4>),
    /// `try`/`catch`/`finally`. `finally` is the arm that is silent when wrong -- see the
    /// conformance README on why that makes it the dangerous one -- so it is always emitted.
    Try(Many<Statement, 3>, Many<Statement, 3>, Many<Statement, 3>),
    /// `throw`, so the `catch` arms above have something to catch.
    Throw(Expression),
}

/// One generated program: two function bodies and a statement list, plus the fixed preamble and
/// epilogue that make it observable.
#[derive(Debug, Clone, Arbitrary)]
pub struct JsProgram {
    initialisers: [Expression; VARIABLES.len()],
    functions: [(Expression, Many<Statement, 3>); FUNCTIONS.len()],
    body: Many<Statement, 6>,
}

/// Turns a generated tree into source text.
///
/// Nothing here is allowed to *shrink* the tree. An earlier version capped expression depth at
/// render time, and measurement showed that four kilobytes of input produced a two-hundred-byte
/// program: the entropy went into subtrees that were then thrown away, and no amount of extra
/// input made the programs any bigger. Depth is bounded where the value is made instead -- see
/// [`crate::Nested`] -- and this only walks what it is given.
struct Renderer {
    out: String,
    /// `Some(name)` inside a loop body.
    induction: Option<&'static str>,
    /// True while rendering a function body.
    ///
    /// Calls are erased there, and that is a termination guarantee rather than a simplification:
    /// `f0` is free to generate `f0(...)`, and unbounded recursion is answered by the
    /// interpreter with `max_call_depth` but by a native binary with a stack overflow. One
    /// engine reporting an error while the other dies of a signal is a divergence the harness
    /// would report thousands of times and that says nothing about the compiler.
    in_function: bool,
}

const INDUCTION_NAMES: [&str; MAX_STATEMENT_DEPTH as usize] = ["i0", "i1"];

impl Renderer {
    fn expression(&mut self, expression: &Expression, depth: u32) {
        if depth >= MAX_EXPRESSION_DEPTH {
            // Bottoming out at a literal rather than at nothing keeps the surrounding syntax
            // valid whatever the parent variant was. Parenthesised, because a bare `1` under a
            // member access renders `1.p0`, which is a syntax error and not the one being
            // looked for -- that alone accounted for a sixth of the corpus before it was fixed.
            self.out.push_str("(1)");
            return;
        }
        match expression {
            Expression::Literal(literal) => self.literal(literal),
            Expression::Variable(slot) => {
                // Inside a function the parameters join the pool, so a call's arguments are
                // reachable. Without this every generated function ignores what it was passed
                // and the call graph carries no data.
                let pool: &[&str] = if self.in_function {
                    &PARAMETERS
                } else {
                    &VARIABLES
                };
                self.out.push_str(pool[*slot as usize % pool.len()]);
            }
            Expression::Induction => match self.induction {
                Some(name) => self.out.push_str(name),
                None => self.out.push_str("(0)"),
            },
            Expression::Unary(op, operand) => {
                self.out.push('(');
                self.out.push_str(op.spelling());
                self.expression(operand, depth + 1);
                self.out.push(')');
            }
            Expression::Binary(op, left, right) => {
                self.out.push('(');
                self.expression(left, depth + 1);
                let _ = write!(self.out, " {} ", op.spelling());
                self.expression(right, depth + 1);
                self.out.push(')');
            }
            Expression::Logical(op, left, right) => {
                self.out.push('(');
                self.expression(left, depth + 1);
                self.out.push_str(match op {
                    LogicalOp::And => " && ",
                    LogicalOp::Or => " || ",
                    LogicalOp::Nullish => " ?? ",
                });
                self.expression(right, depth + 1);
                self.out.push(')');
            }
            Expression::Conditional(test, consequent, alternate) => {
                self.out.push('(');
                self.expression(test, depth + 1);
                self.out.push_str(" ? ");
                self.expression(consequent, depth + 1);
                self.out.push_str(" : ");
                self.expression(alternate, depth + 1);
                self.out.push(')');
            }
            Expression::Array(elements) => {
                self.out.push('[');
                for (index, element) in elements.iter().take(3).enumerate() {
                    if index > 0 {
                        self.out.push_str(", ");
                    }
                    self.expression(element, depth + 1);
                }
                self.out.push(']');
            }
            Expression::Object(values) => {
                self.out.push('{');
                for (index, value) in values.iter().take(3).enumerate() {
                    if index > 0 {
                        self.out.push_str(", ");
                    }
                    let _ = write!(self.out, "p{index}: ");
                    self.expression(value, depth + 1);
                }
                self.out.push('}');
            }
            Expression::Member(object, key) => {
                // `?? {}` on the receiver, and it is worth an explanation. Without it a quarter
                // of every generated corpus died on `undefined.p0` -- measured: three quarters
                // of all generated programs stopped in the interpreter, and reading a property
                // of a nullish value was far and away the largest single cause. A program that
                // throws on its second line compares two engines over two lines of output. The
                // TypeError itself is not lost; `Statement::Throw` still produces uncaught
                // errors, and this only stops property reads from being the *only* thing the
                // corpus ever does.
                self.out.push_str("(((");
                self.expression(object, depth + 1);
                self.out.push_str(") ?? {})");
                self.out.push_str(match key % 3 {
                    0 => ".p0",
                    1 => ".p1",
                    _ => ".length",
                });
                self.out.push(')');
            }
            Expression::Computed(object, key) => {
                // Nullish-guarded for the same reason as `Member` above.
                self.out.push_str("(((");
                self.expression(object, depth + 1);
                self.out.push_str(") ?? {})[");
                self.expression(key, depth + 1);
                self.out.push_str("])");
            }
            Expression::Call(_, first, _) if self.in_function => {
                self.expression(first, depth + 1);
            }
            Expression::Call(callee, first, second) => {
                let _ = write!(
                    self.out,
                    "{}(",
                    FUNCTIONS[*callee as usize % FUNCTIONS.len()]
                );
                self.expression(first, depth + 1);
                self.out.push_str(", ");
                self.expression(second, depth + 1);
                self.out.push(')');
            }
            Expression::Template(inner) => {
                self.out.push_str("`t${");
                self.expression(inner, depth + 1);
                self.out.push_str("}`");
            }
            Expression::Builtin(call, argument) => {
                let _ = write!(self.out, "{}(", call.spelling());
                self.expression(argument, depth + 1);
                self.out.push(')');
            }
        }
    }

    fn literal(&mut self, literal: &Literal) {
        match literal {
            // Parenthesised because `-0` and `-Infinity` are unary expressions, and an unwrapped
            // one changes how the surrounding operator binds.
            Literal::Number(index) => {
                let _ = write!(self.out, "({})", NUMBERS[*index as usize % NUMBERS.len()]);
            }
            Literal::Str(index) => self.out.push_str(STRINGS[*index as usize % STRINGS.len()]),
            Literal::Bool(true) => self.out.push_str("true"),
            Literal::Bool(false) => self.out.push_str("false"),
            Literal::Null => self.out.push_str("null"),
            Literal::Undefined => self.out.push_str("undefined"),
        }
    }

    fn statements(&mut self, statements: &[Statement], depth: u32, indent: usize) {
        // A cap on breadth as well as depth: entropy spent on the hundredth sibling statement
        // buys nothing a fifth one did not.
        for statement in statements.iter().take(6) {
            self.statement(statement, depth, indent);
        }
    }

    fn statement(&mut self, statement: &Statement, depth: u32, indent: usize) {
        let pad = "  ".repeat(indent);
        match statement {
            Statement::Log(expression) => {
                let _ = write!(self.out, "{pad}console.log(");
                self.expression(expression, 0);
                self.out.push_str(");\n");
            }
            Statement::Assign(slot, expression) => {
                let _ = write!(
                    self.out,
                    "{pad}{} = ",
                    VARIABLES[*slot as usize % VARIABLES.len()]
                );
                self.expression(expression, 0);
                self.out.push_str(";\n");
            }
            Statement::Throw(expression) => {
                let _ = write!(self.out, "{pad}throw ");
                self.expression(expression, 0);
                self.out.push_str(";\n");
            }
            // At the depth cap a compound statement degrades to its first arm rather than
            // vanishing, so the entropy already spent on the body is not thrown away.
            Statement::If(test, consequent, alternate) => {
                if depth >= MAX_STATEMENT_DEPTH {
                    self.statements(consequent, depth, indent);
                    return;
                }
                let _ = write!(self.out, "{pad}if (");
                self.expression(test, 0);
                self.out.push_str(") {\n");
                self.statements(consequent, depth + 1, indent + 1);
                let _ = writeln!(self.out, "{pad}}} else {{");
                self.statements(alternate, depth + 1, indent + 1);
                let _ = writeln!(self.out, "{pad}}}");
            }
            Statement::For(bound, body) => {
                if depth >= MAX_STATEMENT_DEPTH {
                    self.statements(body, depth, indent);
                    return;
                }
                let name = INDUCTION_NAMES[depth as usize % INDUCTION_NAMES.len()];
                let trips = u32::from(*bound) % (LOOP_BOUND + 1);
                let _ = writeln!(
                    self.out,
                    "{pad}for (let {name} = 0; {name} < {trips}; {name}++) {{"
                );
                let outer = self.induction.replace(name);
                self.statements(body, depth + 1, indent + 1);
                self.induction = outer;
                let _ = writeln!(self.out, "{pad}}}");
            }
            Statement::Try(block, handler, finaliser) => {
                if depth >= MAX_STATEMENT_DEPTH {
                    self.statements(block, depth, indent);
                    return;
                }
                let _ = writeln!(self.out, "{pad}try {{");
                self.statements(block, depth + 1, indent + 1);
                let _ = writeln!(self.out, "{pad}}} catch (e) {{");
                self.statements(handler, depth + 1, indent + 1);
                let _ = writeln!(self.out, "{pad}}} finally {{");
                self.statements(finaliser, depth + 1, indent + 1);
                let _ = writeln!(self.out, "{pad}}}");
            }
        }
    }
}

impl JsProgram {
    /// Render this program to a syntactically valid script.
    ///
    /// The epilogue is not decoration. A program whose whole effect is on `a`..`d` prints
    /// nothing, and two engines that print nothing agree trivially; logging the final state
    /// turns every generated program into an observation.
    pub fn render(&self) -> String {
        let mut renderer = Renderer {
            out: String::new(),
            induction: None,
            in_function: false,
        };

        // Declare first, initialise after. `let a = b + 1` is a temporal-dead-zone access, and
        // lowering is right to refuse it -- but a generator that spends a third of its corpus
        // rediscovering that is measuring its own bug. Splitting the two makes every name in
        // scope before any initialiser runs.
        let _ = writeln!(renderer.out, "let {};", VARIABLES.join(", "));
        for (name, initialiser) in VARIABLES.iter().zip(&self.initialisers) {
            let _ = write!(renderer.out, "{name} = ");
            renderer.expression(initialiser, 1);
            renderer.out.push_str(";\n");
        }
        for (name, (result, body)) in FUNCTIONS.iter().zip(&self.functions) {
            // Parameters shadow nothing: `p`/`q` are not in `VARIABLES`, so a body is free to
            // read the outer bindings too.
            let _ = writeln!(renderer.out, "function {name}(p, q) {{");
            renderer.in_function = true;
            renderer.statements(body, MAX_STATEMENT_DEPTH - 1, 1);
            renderer.out.push_str("  return ");
            renderer.expression(result, 1);
            renderer.out.push_str(";\n}\n");
            renderer.in_function = false;
        }
        renderer.statements(&self.body, 0, 0);
        for name in VARIABLES {
            let _ = writeln!(renderer.out, "console.log({name});");
        }
        renderer.out
    }
}
