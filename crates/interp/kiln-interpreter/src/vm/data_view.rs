use super::*;

impl<H: HostOps + ?Sized> Vm<'_, H> {
    /// `new DataView(buffer, byteOffset?, byteLength?)`.
    ///
    /// The order matters and is the specification's: the buffer is checked first, then the
    /// offset is coerced, then it is measured against a length read *after* that coercion, and
    /// only then is an explicit byte length coerced and checked. Each coercion can run user
    /// code that resizes the buffer, so a length read once at the top would be stale by the
    /// time it was compared against.
    pub(super) fn construct_data_view(
        &mut self,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Some(Value::ArrayBuffer(buffer)) = arguments.first() else {
            return Err(ExecutionError::NotABuffer);
        };
        let buffer = Rc::clone(buffer);
        // After the offset is coerced, not before: the specification's order is observable
        // through a `valueOf` that runs even though the view can never be built.
        let offset = self.as_index_argument(arguments.get(1), instruction)?;
        if buffer.detached() {
            return Err(ExecutionError::BufferDetached);
        }
        if offset > buffer.byte_length() {
            return Err(ExecutionError::DataViewOutOfRange);
        }
        let length = match arguments.get(2).cloned() {
            // No explicit length over a *resizable* buffer is the specification's
            // length-tracking view, the same reading `new Uint8Array(rab)` gets: its end
            // follows the buffer's rather than freezing at today's length.
            None | Some(Value::Undefined) if buffer.max_byte_length.is_some() => None,
            None | Some(Value::Undefined) => Some(buffer.byte_length() - offset),
            Some(value) => {
                let length = self.as_index_argument(Some(&value), instruction)?;
                let total = buffer.byte_length();
                if offset > total || length > total - offset {
                    return Err(ExecutionError::DataViewOutOfRange);
                }
                Some(length)
            }
        };
        Ok(Value::DataView(Rc::new(DataViewData {
            buffer,
            offset,
            length,
            properties: RefCell::new(ObjectData::default()),
        })))
    }

    /// `view.getInt16(byteOffset, littleEndian?)` and its seven siblings.
    ///
    /// The byte-order argument defaults to *big*-endian -- the opposite of every typed array and
    /// of every platform this runs on. That is deliberate in the specification: a `DataView`
    /// exists to read data that came from somewhere else, and somewhere else usually means
    /// network byte order. It is also the single detail an implementation is most likely to get
    /// backwards, because the default costs a byte reversal on every ordinary read.
    pub(super) fn data_view_get(
        &mut self,
        kind: TypedKind,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::DataView(view) = receiver else {
            return Err(ExecutionError::NotADataView);
        };
        let view = Rc::clone(view);
        let index = self.as_index_argument(arguments.first(), instruction)?;
        let little_endian = arguments.get(1).is_some_and(Value::is_truthy);
        let width = kind.width();
        let at = data_view_slot(&view, index, width)?;
        let mut bytes = [0_u8; 8];
        bytes[..width].copy_from_slice(&view.buffer.bytes.borrow()[at..at + width]);
        order_bytes(&mut bytes[..width], little_endian);
        Ok(kind.decode(&bytes[..width]))
    }

    /// `view.setInt16(byteOffset, value, littleEndian?)` and its seven siblings.
    pub(super) fn data_view_set(
        &mut self,
        kind: TypedKind,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::DataView(view) = receiver else {
            return Err(ExecutionError::NotADataView);
        };
        let view = Rc::clone(view);
        let index = self.as_index_argument(arguments.first(), instruction)?;
        // Converted here, before the bounds are checked and before the byte-order argument is
        // even looked at, because that is where the specification puts it: a `valueOf` that
        // shrinks the buffer has to be able to make the write that follows fail.
        let value = self.as_number_argument(arguments.get(1), instruction)?;
        let little_endian = arguments.get(2).is_some_and(Value::is_truthy);
        let width = kind.width();
        let at = data_view_slot(&view, index, width)?;
        let mut bytes = [0_u8; 8];
        kind.encode(&Value::Number(value), &mut bytes[..width]);
        order_bytes(&mut bytes[..width], little_endian);
        view.buffer.bytes.borrow_mut()[at..at + width].copy_from_slice(&bytes[..width]);
        Ok(Value::Undefined)
    }

    /// `view.getBigInt64(byteOffset, littleEndian?)` and its unsigned pair.
    ///
    /// Apart from the eight numeric accessors because the value is: a sixty-four bit integer
    /// does not fit in the `f64` every other element type decodes to, and rounding one to the
    /// nearest double is exactly the silent loss the BigInt type exists to prevent. These read
    /// and write a `BigInt`, so the bytes survive the round trip exactly.
    pub(super) fn data_view_get_big(
        &mut self,
        signed: bool,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::DataView(view) = receiver else {
            return Err(ExecutionError::NotADataView);
        };
        let view = Rc::clone(view);
        let index = self.as_index_argument(arguments.first(), instruction)?;
        let little_endian = arguments.get(1).is_some_and(Value::is_truthy);
        let at = data_view_slot(&view, index, 8)?;
        let mut bytes = [0_u8; 8];
        bytes.copy_from_slice(&view.buffer.bytes.borrow()[at..at + 8]);
        order_bytes(&mut bytes, little_endian);
        let raw = u64::from_le_bytes(bytes);
        Ok(Value::BigInt(Rc::new(if signed {
            num_bigint::BigInt::from(raw as i64)
        } else {
            num_bigint::BigInt::from(raw)
        })))
    }

    /// `view.setBigInt64(byteOffset, value, littleEndian?)` and its unsigned pair.
    pub(super) fn data_view_set_big(
        &mut self,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::DataView(view) = receiver else {
            return Err(ExecutionError::NotADataView);
        };
        let view = Rc::clone(view);
        let index = self.as_index_argument(arguments.first(), instruction)?;
        let value = self.as_big_int_argument(arguments.get(1), instruction)?;
        let little_endian = arguments.get(2).is_some_and(Value::is_truthy);
        let at = data_view_slot(&view, index, 8)?;
        let mut bytes = wrapping_u64(&value).to_le_bytes();
        order_bytes(&mut bytes, little_endian);
        view.buffer.bytes.borrow_mut()[at..at + 8].copy_from_slice(&bytes);
        Ok(Value::Undefined)
    }
}
