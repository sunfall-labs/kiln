//! How many bytes does one generated program actually consume?
use arbitrary::{Arbitrary, Unstructured};
use kiln_fuzz::ir::IrProgram;
use kiln_fuzz::js::JsProgram;

fn main() {
    let mut seed = 0x1234_5678_9abc_def0_u64;
    let mut next = move || {
        seed ^= seed >> 12;
        seed ^= seed << 25;
        seed ^= seed >> 27;
        seed.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    for len in [64usize, 256, 1024, 4096] {
        let mut bytes = vec![0u8; len];
        for chunk in bytes.chunks_mut(8) {
            let v = next().to_le_bytes();
            chunk.copy_from_slice(&v[..chunk.len()]);
        }
        let mut u = Unstructured::new(&bytes);
        let program = JsProgram::arbitrary(&mut u).unwrap();
        let consumed = len - u.len();
        let source = program.render();
        let mut u2 = Unstructured::new(&bytes);
        let ir = IrProgram::arbitrary(&mut u2).unwrap();
        println!(
            "len {len:>5}: js consumed {consumed:>5}, rendered {:>5} bytes; ir {:>3} instructions",
            source.len(),
            ir.build().0.code.len()
        );
    }
}
