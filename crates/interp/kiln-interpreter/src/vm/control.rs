use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    pub(super) fn pop(&mut self, instruction: usize) -> Result<Value, ExecutionError> {
        self.stack
            .pop()
            .ok_or(ExecutionError::StackUnderflow { instruction })
    }

    pub(super) fn jump(&mut self, instruction: usize, target: u32) -> Result<(), ExecutionError> {
        let target = target as usize;
        if target >= Self::code(&self.program, self.function.as_ref()).len() {
            return Err(ExecutionError::InvalidJump {
                instruction,
                target,
            });
        }
        self.instruction = target;
        Ok(())
    }
}
