use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// `ToLength(argument)`: the bound a generic walk runs to.
    pub(super) fn to_length(number: f64) -> u64 {
        if number.is_nan() || number <= 0.0 {
            return 0;
        }
        let integer = number.trunc();
        if integer >= MAX_SAFE_INTEGER {
            MAX_SAFE_INTEGER_AS_U64
        } else {
            #[expect(clippy::cast_possible_truncation)]
            let bounded = integer as u64;
            bounded
        }
    }

    /// `ToIntegerOrInfinity`, kept as an `f64` so an infinite argument stays infinite: the
    /// difference between `slice(-Infinity)` and `slice(-1)` is the whole array or one element.
    pub(super) fn to_integer_or_infinity(number: f64) -> f64 {
        if number.is_nan() { 0.0 } else { number.trunc() }
    }

    /// A `start`/`end` argument resolved against a length: negative counts from the end, and
    /// anything past either edge clamps to it.
    pub(super) fn relative_bound(relative: f64, length: u64) -> u64 {
        #[expect(clippy::cast_precision_loss)]
        let span = length as f64;
        let resolved = if relative < 0.0 {
            (span + relative).max(0.0)
        } else {
            relative.min(span)
        };
        #[expect(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let resolved = resolved as u64;
        resolved
    }

    /// One step of a generic walk, refusing once a walk has taken more of them than this
    /// implementation will materialise an array of.
    ///
    /// `Array.prototype.indexOf.call({length: 2 ** 32}, x)` is a four-billion-step loop the
    /// specification really does ask for, and the corpus passes such objects on purpose --
    /// always arranging for the answer to be found early, which is why the count is checked per
    /// step rather than the length checked up front. A walk that genuinely runs to the end of
    /// one is a hang, and refusing it is the same limit this implementation already reports for
    /// an array that large.
    pub(super) fn walk_step(steps: &mut u64) -> Result<(), ExecutionError> {
        *steps += 1;
        if *steps > MAX_DENSE_ARRAY as u64 {
            return Err(ExecutionError::ArrayTooLarge {
                length: MAX_DENSE_ARRAY,
            });
        }
        Ok(())
    }

    /// The key an index names. A number rather than its spelling, so an array or a view can
    /// answer without the string ever being built.
    pub(super) fn element_key(index: u64) -> Value {
        #[expect(clippy::cast_precision_loss)]
        let index = index as f64;
        Value::Number(index)
    }
}
