//! One JSON grammar over JavaScript UTF-16 units.

use crate::{
    Node, ParseError, TextArena, UnitSource, TAG_ARRAY, TAG_FALSE, TAG_NULL, TAG_NUMBER,
    TAG_OBJECT, TAG_STRING, TAG_TRUE,
};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame {
    pub(crate) node: usize,
    pub(crate) children: u32,
    pub(crate) is_object: bool,
}

const EXACT_POW10: [f64; 23] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
    1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
];

pub(crate) fn number_from_parts(
    negative: bool,
    mantissa: u64,
    significant: u32,
    exponent: Option<i32>,
    token: &[u8],
) -> f64 {
    let fast = match exponent {
        Some(_) if mantissa == 0 => Some(0.0),
        Some(exponent) if significant <= 15 && (-22..=22).contains(&exponent) => {
            let base = mantissa as f64;
            Some(if exponent < 0 {
                base / EXACT_POW10[(-exponent) as usize]
            } else {
                base * EXACT_POW10[exponent as usize]
            })
        }
        _ => None,
    };
    let magnitude = fast.unwrap_or_else(|| {
        std::str::from_utf8(token)
            .ok()
            .and_then(|text| text.parse::<f64>().ok())
            .unwrap_or(f64::NAN)
    });
    if negative {
        -magnitude
    } else {
        magnitude
    }
}

fn scan_number<S: UnitSource>(
    source: &S,
    start: usize,
    token: &mut Vec<u8>,
) -> Result<(usize, f64), ParseError> {
    let mut at = start;
    let negative = source.unit_or_end(at) == u32::from(b'-');
    if negative {
        at += 1;
    }
    let push = |index: usize| -> Result<(), ParseError> {
        let unit = source.unit_or_end(index);
        if unit > 0x7f {
            return Err(ParseError::Syntax { offset: index });
        }
        Ok(())
    };
    let mut mantissa = 0_u64;
    let mut significant = 0_u32;
    let mut scale = 0_i32;
    match source.unit_or_end(at) {
        0x30 => {
            push(at)?;
            at += 1;
        }
        0x31..=0x39 => {
            while let unit @ 0x30..=0x39 = source.unit_or_end(at) {
                significant += 1;
                if significant <= 15 {
                    mantissa = mantissa * 10 + u64::from(unit - 0x30);
                }
                at += 1;
            }
        }
        _ => return Err(ParseError::Syntax { offset: at }),
    }
    if source.unit_or_end(at) == 0x2e {
        at += 1;
        let fraction_start = at;
        while let unit @ 0x30..=0x39 = source.unit_or_end(at) {
            if significant > 0 || unit != 0x30 {
                significant += 1;
                if significant <= 15 {
                    mantissa = mantissa * 10 + u64::from(unit - 0x30);
                }
            }
            scale -= 1;
            at += 1;
        }
        if at == fraction_start {
            return Err(ParseError::Syntax { offset: at });
        }
    }
    let mut exponent = Some(scale);
    if matches!(source.unit_or_end(at), 0x65 | 0x45) {
        at += 1;
        let exponent_negative = match source.unit_or_end(at) {
            0x2d => {
                push(at)?;
                at += 1;
                true
            }
            0x2b => {
                push(at)?;
                at += 1;
                false
            }
            _ => false,
        };
        let exponent_start = at;
        let mut explicit = Some(0_i32);
        while let unit @ 0x30..=0x39 = source.unit_or_end(at) {
            explicit = explicit
                .and_then(|value| {
                    value
                        .checked_mul(10)?
                        .checked_add(i32::from((unit - 0x30) as u8))
                })
                .filter(|value| *value <= 400);
            at += 1;
        }
        if at == exponent_start {
            return Err(ParseError::Syntax { offset: at });
        }
        exponent = explicit
            .and_then(|value| scale.checked_add(if exponent_negative { -value } else { value }));
    }
    let fast = (exponent.is_some() && mantissa == 0)
        || matches!(exponent, Some(value) if significant <= 15 && (-22..=22).contains(&value));
    token.clear();
    if !fast {
        for index in (start + usize::from(negative))..at {
            push(index)?;
            token.push(source.unit_or_end(index) as u8);
        }
    }
    let magnitude = &token[..];
    Ok((
        at,
        number_from_parts(negative, mantissa, significant, exponent, magnitude),
    ))
}

#[inline]
fn skip_whitespace<S: UnitSource>(source: &S, mut at: usize) -> usize {
    while matches!(source.unit_or_end(at), 0x20 | 0x09 | 0x0a | 0x0d) {
        at += 1;
    }
    at
}

#[inline]
fn hex4<S: UnitSource>(source: &S, at: usize) -> Option<u16> {
    let mut unit = 0_u16;
    for index in 0..4 {
        let digit = match source.unit_or_end(at + index) {
            0x30..=0x39 => source.unit_or_end(at + index) as u16 - 0x30,
            0x61..=0x66 => source.unit_or_end(at + index) as u16 - 0x61 + 10,
            0x41..=0x46 => source.unit_or_end(at + index) as u16 - 0x41 + 10,
            _ => return None,
        };
        unit = (unit << 4) | digit;
    }
    Some(unit)
}

fn decode_string<S: UnitSource>(
    source: &S,
    open: usize,
    arena: &mut TextArena,
) -> Result<usize, ParseError> {
    let mut at = open + 1;
    let raw_start = at;
    if let Some(bytes) = source.latin1_slice(raw_start, source.unit_len()) {
        for (index, &byte) in bytes.iter().enumerate() {
            match byte {
                0x22 => {
                    arena.extend_latin1(&bytes[..index]);
                    return Ok(raw_start + index + 1);
                }
                0x5c | 0x00..=0x1f => break,
                _ => {}
            }
        }
    }
    at = raw_start;
    loop {
        let unit = source.unit_or_end(at);
        if unit == 0x1_0000 {
            return Err(ParseError::Syntax { offset: at });
        }
        match unit {
            0x22 => return Ok(at + 1),
            0x00..=0x1f => return Err(ParseError::Syntax { offset: at }),
            0x5c => {
                let kind_at = at + 1;
                let kind = source.unit_or_end(kind_at);
                if kind == 0x1_0000 {
                    return Err(ParseError::Syntax { offset: at });
                }
                at += 2;
                match kind {
                    0x22 | 0x5c | 0x2f => arena.push_unit(kind as u16),
                    0x62 => arena.push_unit(0x08),
                    0x66 => arena.push_unit(0x0c),
                    0x6e => arena.push_unit(0x0a),
                    0x72 => arena.push_unit(0x0d),
                    0x74 => arena.push_unit(0x09),
                    0x75 => {
                        let unit = hex4(source, at).ok_or(ParseError::Syntax { offset: at })?;
                        at += 4;
                        arena.push_unit(unit);
                    }
                    _ => return Err(ParseError::Syntax { offset: kind_at }),
                }
            }
            _ => {
                arena.push_unit(unit as u16);
                at += 1;
            }
        }
    }
}

#[inline]
fn string_node<S: UnitSource>(
    source: &S,
    at: usize,
    nodes: &mut Vec<Node>,
    arena: &mut TextArena,
) -> Result<usize, ParseError> {
    let offset = arena.len();
    let end = decode_string(source, at, arena)?;
    let len = arena
        .len()
        .checked_sub(offset)
        .ok_or(ParseError::TooLarge)?;
    nodes.push(Node {
        tag: TAG_STRING,
        len: u32::try_from(len).map_err(|_| ParseError::TooLarge)?,
        value: offset as u64,
    });
    Ok(end)
}

fn member_key<S: UnitSource>(
    source: &S,
    at: usize,
    nodes: &mut Vec<Node>,
    arena: &mut TextArena,
) -> Result<usize, ParseError> {
    if source.unit_or_end(at) != 0x22 {
        return Err(ParseError::Syntax { offset: at });
    }
    let after_key = string_node(source, at, nodes, arena)?;
    let colon = skip_whitespace(source, after_key);
    if source.unit_or_end(colon) != u32::from(b':') {
        return Err(ParseError::Syntax { offset: colon });
    }
    Ok(skip_whitespace(source, colon + 1))
}

#[inline]
fn literal<S: UnitSource>(
    source: &S,
    at: usize,
    word: &[u8],
    tag: u32,
    nodes: &mut Vec<Node>,
) -> Result<usize, ParseError> {
    for (index, &byte) in word.iter().enumerate() {
        if source.unit_or_end(at + index) != u32::from(byte) {
            return Err(ParseError::Syntax { offset: at });
        }
    }
    nodes.push(Node {
        tag,
        len: 0,
        value: 0,
    });
    Ok(at + word.len())
}

pub(crate) fn scan<S: UnitSource>(
    source: &S,
    nodes: &mut Vec<Node>,
    arena: &mut TextArena,
    stack: &mut Vec<Frame>,
    token: &mut Vec<u8>,
) -> Result<(), ParseError> {
    nodes.clear();
    stack.clear();
    arena.clear();
    let mut at = skip_whitespace(source, 0);
    'value: loop {
        let unit = source.unit_or_end(at);
        if unit == 0x1_0000 {
            return Err(ParseError::Syntax { offset: at });
        }
        match unit {
            0x22 => at = string_node(source, at, nodes, arena)?,
            0x7b | 0x5b => {
                let is_object = unit == 0x7b;
                let node = nodes.len();
                nodes.push(Node {
                    tag: if is_object { TAG_OBJECT } else { TAG_ARRAY },
                    len: 0,
                    value: 0,
                });
                at = skip_whitespace(source, at + 1);
                let close = if is_object { 0x7d } else { 0x5d };
                if source.unit_or_end(at) == close {
                    at += 1;
                } else {
                    stack.push(Frame {
                        node,
                        children: 0,
                        is_object,
                    });
                    if is_object {
                        at = member_key(source, at, nodes, arena)?;
                    }
                    continue 'value;
                }
            }
            0x74 => at = literal(source, at, b"true", TAG_TRUE, nodes)?,
            0x66 => at = literal(source, at, b"false", TAG_FALSE, nodes)?,
            0x6e => at = literal(source, at, b"null", TAG_NULL, nodes)?,
            0x2d | 0x30..=0x39 => {
                let (end, value) = scan_number(source, at, token)?;
                nodes.push(Node {
                    tag: TAG_NUMBER,
                    len: 0,
                    value: value.to_bits(),
                });
                at = end;
            }
            _ => return Err(ParseError::Syntax { offset: at }),
        }
        loop {
            let Some(frame) = stack.last_mut() else {
                at = skip_whitespace(source, at);
                return if at == source.unit_len() {
                    Ok(())
                } else {
                    Err(ParseError::Syntax { offset: at })
                };
            };
            frame.children = frame.children.checked_add(1).ok_or(ParseError::TooLarge)?;
            at = skip_whitespace(source, at);
            let next = source.unit_or_end(at);
            if next == 0x1_0000 {
                return Err(ParseError::Syntax { offset: at });
            }
            if next == 0x2c {
                at = skip_whitespace(source, at + 1);
                if frame.is_object {
                    at = member_key(source, at, nodes, arena)?;
                }
                continue 'value;
            }
            let close = if frame.is_object { 0x7d } else { 0x5d };
            if next != close {
                return Err(ParseError::Syntax { offset: at });
            }
            nodes[frame.node].len = frame.children;
            stack.pop();
            at += 1;
        }
    }
}
