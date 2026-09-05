//! Generator of small typed-ish JavaScript programs for differential fuzzing of the optimizer.
//!
//! As specified in Phase 2 Step 2.2 of `fastest-native-plan.md`:
//! - Generates programs with numeric parameters, unboxed double operations, numeric loops, and Math builtins.
//! - Exercises both speculative fast paths (all-double calls) and speculation exits (calls with non-numeric inputs).
//! - All generated programs are deterministic, terminating, and syntactically valid JavaScript.

use arbitrary::Arbitrary;
use std::fmt::Write as _;

/// A small typed-ish JavaScript program designed to exercise numeric speculation and guard exits.
#[derive(Debug, Clone, Arbitrary)]
pub struct TypedProgram {
    pub functions: Vec<TypedFunction>,
    pub invocations: Vec<Invocation>,
}

#[derive(Debug, Clone, Arbitrary)]
pub struct TypedFunction {
    pub arity: u8,
    pub operations: Vec<TypedOp>,
    pub has_loop: bool,
    pub loop_bound: u8,
}

#[derive(Debug, Clone, Copy, Arbitrary)]
pub enum TypedOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    BitOr,
    BitAnd,
    BitXor,
    MathAbs,
    MathFloor,
    MathMin,
    MathMax,
}

#[derive(Debug, Clone, Arbitrary)]
pub enum Invocation {
    PureNumeric {
        func_index: u8,
        args: Vec<f64>,
    },
    SpeculationExit {
        func_index: u8,
        exit_slot: u8,
        non_num: NonNumericVal,
    },
}

#[derive(Debug, Clone, Copy, Arbitrary)]
pub enum NonNumericVal {
    String,
    Undefined,
    Null,
    Boolean,
}

impl TypedProgram {
    /// Render the program as executable JavaScript source.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let num_funcs = (self.functions.len() % 4).max(1);

        for f_idx in 0..num_funcs {
            let func = &self.functions[f_idx % self.functions.len()];
            let arity = ((func.arity % 3) + 1) as usize;
            let param_names: Vec<String> = (0..arity).map(|i| format!("p{i}")).collect();
            let params_str = param_names.join(", ");

            writeln!(out, "function fn{f_idx}({params_str}) {{").unwrap();
            writeln!(out, "    let acc = p0;").unwrap();

            if func.has_loop {
                let bound = (func.loop_bound % 5) + 1;
                writeln!(out, "    for (let i = 0; i < {bound}; i++) {{").unwrap();
                writeln!(out, "        acc = acc + i * 1.5;").unwrap();
                writeln!(out, "    }}").unwrap();
            }

            let num_ops = (func.operations.len() % 4).max(1);
            for op_idx in 0..num_ops {
                let op = func.operations[op_idx % func.operations.len()];
                let operand = &param_names[(op_idx + 1) % arity];
                match op {
                    TypedOp::Add => writeln!(out, "    acc = acc + {operand};").unwrap(),
                    TypedOp::Sub => writeln!(out, "    acc = acc - {operand};").unwrap(),
                    TypedOp::Mul => writeln!(out, "    acc = acc * ({operand} + 0.5);").unwrap(),
                    TypedOp::Div => {
                        writeln!(out, "    acc = acc / ({operand} === 0 ? 1.0 : {operand});")
                            .unwrap()
                    }
                    TypedOp::Mod => writeln!(
                        out,
                        "    acc = (acc % ({operand} === 0 ? 3.0 : {operand}));"
                    )
                    .unwrap(),
                    TypedOp::BitOr => {
                        writeln!(out, "    acc = (acc | 0) | ({operand} | 0);").unwrap()
                    }
                    TypedOp::BitAnd => {
                        writeln!(out, "    acc = (acc | 0) & ({operand} | 0);").unwrap()
                    }
                    TypedOp::BitXor => {
                        writeln!(out, "    acc = (acc | 0) ^ ({operand} | 0);").unwrap()
                    }
                    TypedOp::MathAbs => writeln!(out, "    acc = Math.abs(acc);").unwrap(),
                    TypedOp::MathFloor => writeln!(out, "    acc = Math.floor(acc);").unwrap(),
                    TypedOp::MathMin => {
                        writeln!(out, "    acc = Math.min(acc, {operand});").unwrap()
                    }
                    TypedOp::MathMax => {
                        writeln!(out, "    acc = Math.max(acc, {operand});").unwrap()
                    }
                }
            }

            writeln!(out, "    return acc;").unwrap();
            writeln!(out, "}}\n").unwrap();
        }

        // Emit invocations
        let num_invocations = (self.invocations.len() % 6).max(2);
        for inv_idx in 0..num_invocations {
            let inv = &self.invocations[inv_idx % self.invocations.len()];
            match inv {
                Invocation::PureNumeric { func_index, args } => {
                    let f_idx = (*func_index as usize) % num_funcs;
                    let func = &self.functions[f_idx % self.functions.len()];
                    let arity = ((func.arity % 3) + 1) as usize;
                    let mut call_args = Vec::new();
                    for a_idx in 0..arity {
                        let val = args
                            .get(a_idx)
                            .copied()
                            .unwrap_or((a_idx as f64 + 1.0) * 2.5);
                        let clean_val = if val.is_finite() {
                            (val % 100.0).abs() + 0.5
                        } else {
                            2.0
                        };
                        call_args.push(format!("{clean_val:.2}"));
                    }
                    writeln!(
                        out,
                        "console.log(\"call_num_{inv_idx}:\", fn{f_idx}({}));",
                        call_args.join(", ")
                    )
                    .unwrap();
                }
                Invocation::SpeculationExit {
                    func_index,
                    exit_slot,
                    non_num,
                } => {
                    let f_idx = (*func_index as usize) % num_funcs;
                    let func = &self.functions[f_idx % self.functions.len()];
                    let arity = ((func.arity % 3) + 1) as usize;
                    let slot = (*exit_slot as usize) % arity;
                    let non_num_str = match non_num {
                        NonNumericVal::String => "\"fallback\"",
                        NonNumericVal::Undefined => "undefined",
                        NonNumericVal::Null => "null",
                        NonNumericVal::Boolean => "true",
                    };
                    let mut call_args = Vec::new();
                    for a_idx in 0..arity {
                        if a_idx == slot {
                            call_args.push(non_num_str.to_string());
                        } else {
                            call_args.push(format!("{:.1}", (a_idx as f64 + 1.0) * 1.5));
                        }
                    }
                    writeln!(
                        out,
                        "console.log(\"call_exit_{inv_idx}:\", fn{f_idx}({}));",
                        call_args.join(", ")
                    )
                    .unwrap();
                }
            }
        }

        out
    }

    /// Generate a deterministic pseudo-random typed program from a seed.
    pub fn generate_seeded(seed: u64) -> String {
        let mut x = seed;
        let mut next_u64 = || {
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };

        let mut out = String::new();
        let num_funcs = ((next_u64() % 3) + 2) as usize; // 2..4 functions

        for f_idx in 0..num_funcs {
            let arity = ((next_u64() % 3) + 1) as usize; // 1..3 params
            let param_names: Vec<String> = (0..arity).map(|i| format!("p{i}")).collect();
            let params_str = param_names.join(", ");

            writeln!(out, "function fn{f_idx}({params_str}) {{").unwrap();
            writeln!(out, "    let acc = p0;").unwrap();

            let has_loop = (next_u64() % 2) == 1;
            if has_loop {
                let bound = (next_u64() % 4) + 1;
                writeln!(out, "    for (let i = 0; i < {bound}; i++) {{").unwrap();
                writeln!(out, "        acc = acc + (i * 2);").unwrap();
                writeln!(out, "    }}").unwrap();
            }

            let num_ops = ((next_u64() % 3) + 1) as usize;
            for op_idx in 0..num_ops {
                let operand = &param_names[(op_idx + 1) % arity];
                match next_u64() % 8 {
                    0 => writeln!(out, "    acc = acc + {operand};").unwrap(),
                    1 => writeln!(out, "    acc = acc - {operand};").unwrap(),
                    2 => writeln!(out, "    acc = acc * ({operand} + 1);").unwrap(),
                    3 => writeln!(out, "    acc = Math.abs(acc);").unwrap(),
                    4 => writeln!(out, "    acc = Math.min(acc, {operand});").unwrap(),
                    5 => writeln!(out, "    acc = Math.max(acc, {operand});").unwrap(),
                    6 => writeln!(out, "    acc = (acc | 0) + ({operand} | 0);").unwrap(),
                    _ => writeln!(out, "    acc = Math.floor(acc);").unwrap(),
                }
            }

            writeln!(out, "    return acc;").unwrap();
            writeln!(out, "}}\n").unwrap();
        }

        // Both numeric and deopt invocations
        for f_idx in 0..num_funcs {
            let arity = (f_idx % 3) + 1;
            // 1. Pure numeric call
            let num_args: Vec<String> = (0..arity).map(|i| format!("{}.5", i + 1)).collect();
            writeln!(
                out,
                "console.log(\"fn{f_idx}_num:\", fn{f_idx}({}));",
                num_args.join(", ")
            )
            .unwrap();

            // 2. Speculation exit / non-numeric call
            let mut deopt_args = num_args.clone();
            deopt_args[0] = "\"exit\"".to_string();
            writeln!(
                out,
                "console.log(\"fn{f_idx}_exit:\", fn{f_idx}({}));",
                deopt_args.join(", ")
            )
            .unwrap();
        }

        out
    }
}
