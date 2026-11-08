#![forbid(unsafe_code)]

//! The index path technology — a technology of `xmip-core-path`.
//!
//! [`IndexLanguage`] is the [`PathLanguage`] `index`: it compiles an index
//! once, and the compiled index reads one field of a delimited text Stream
//! and writes one into a rewrite of it (ADR-0013). Promote reads through it;
//! demote writes through it; route and process read.
//!
//! An index is `record/field`, both counted from 1, or `field` alone for the
//! first record: `3/2` is the second field of the third record. Records are
//! lines; fields are split on the delimiter the language is configured with,
//! comma unless another is given. Quoting is the CSV contract's concern, not
//! this one's: an index addresses positions, and a value is the text found
//! there. The lines of a Stream are found once per Message, however many
//! indexes read it; the text is the Stream's own, never copied for a read.

use contract::ContractError;
use path::{CompiledExpression, Content, Form, Namespaces, PathLanguage, Rewriting};
use std::ops::Range;
use stream::Stream;
use xcore::ScalarValue;

/// The language `index`, splitting fields on one delimiter.
pub struct IndexLanguage {
    delimiter: char,
}

impl IndexLanguage {
    /// Fields split on `delimiter`.
    #[must_use]
    pub const fn new(delimiter: char) -> Self {
        Self { delimiter }
    }
}

/// Comma, as CSV has it.
impl Default for IndexLanguage {
    fn default() -> Self {
        Self::new(',')
    }
}

impl PathLanguage for IndexLanguage {
    fn language(&self) -> &'static str {
        "index"
    }

    fn compile(
        &self,
        expression: &str,
        namespaces: &Namespaces,
    ) -> Result<Box<dyn CompiledExpression>, ContractError> {
        namespaces.unused_by(self.language())?;
        let (record, field) = record_and_field(expression)?;
        Ok(Box::new(Compiled {
            record,
            field,
            delimiter: self.delimiter,
        }))
    }
}

/// `record/field` or `field`, both from 1.
fn record_and_field(expression: &str) -> Result<(usize, usize), ContractError> {
    let refuse = || {
        ContractError::new(format!(
            "{expression:?} is not record/field or field, counted from 1"
        ))
    };
    let (record, field) = match expression.split_once('/') {
        Some((record, field)) => (record.trim().parse().map_err(|_| refuse())?, field.trim()),
        None => (1, expression.trim()),
    };
    let field: usize = field.parse().map_err(|_| refuse())?;
    if record == 0 || field == 0 {
        return Err(refuse());
    }
    Ok((record, field))
}

/// Where each line of `text` is, its line break left out.
fn line_ranges(text: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    let mut start = 0;
    text.split_inclusive('\n').map(move |line| {
        let begin = start;
        start += line.len();
        let body = line.strip_suffix('\n').unwrap_or(line);
        let body = body.strip_suffix('\r').unwrap_or(body);
        begin..begin + body.len()
    })
}

/// The lines of a text Stream, found once: where each is, not a copy of it.
struct Lines(Vec<Range<usize>>);

impl Form for Lines {
    fn parse(stream: &Stream) -> Result<Self, ContractError> {
        let text = path::text(stream)?;
        Ok(Self(line_ranges(text).collect()))
    }
}

/// An index compiled.
struct Compiled {
    record: usize,
    field: usize,
    delimiter: char,
}

impl CompiledExpression for Compiled {
    fn read(&self, content: &Content<'_>) -> Result<Option<ScalarValue>, ContractError> {
        let lines = content.form::<Lines>()?;
        let text = content.text()?;
        Ok(lines
            .0
            .get(self.record - 1)
            .and_then(|line| text[line.clone()].split(self.delimiter).nth(self.field - 1))
            .map(|found| ScalarValue::Text(found.to_string())))
    }

    /// Replace one field. A record that does not exist is refused; a field
    /// past the end of an existing record is appended, padding with empties,
    /// because a delimited record has no declared width to violate.
    fn write(&self, rewriting: &mut Rewriting, value: ScalarValue) -> Result<(), ContractError> {
        let written = lexical(&value, self.delimiter)?;
        let text = rewriting.form_mut::<String>()?;
        let line = line_ranges(text)
            .nth(self.record - 1)
            .ok_or_else(|| ContractError::new(format!("record {} does not exist", self.record)))?;
        let mut fields: Vec<&str> = text[line.clone()].split(self.delimiter).collect();
        if fields.len() < self.field {
            fields.resize(self.field, "");
        }
        fields[self.field - 1] = &written;
        let record = fields.join(self.delimiter.encode_utf8(&mut [0; 4]));
        text.replace_range(line, &record);
        Ok(())
    }
}

/// The text a value is written as; a Null is an empty field. A value
/// carrying the delimiter or a line break would change the record count for
/// every reader after us, so it is refused rather than quoted: quoting is the
/// CSV contract's dialect.
fn lexical(value: &ScalarValue, delimiter: char) -> Result<String, ContractError> {
    let text = match value {
        ScalarValue::Null => String::new(),
        other => other
            .text()
            .ok_or_else(|| ContractError::new("binary has no text form here"))?
            .into_owned(),
    };
    if text.contains(delimiter) || text.contains(['\n', '\r']) {
        return Err(ContractError::new(format!(
            "{text:?} would break the record it is written into"
        )));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use contract::fixture::stream;
    use xcore::StreamId;

    const ORDERS: &str = "id,customer,total\nA1,EXAMPLE,15.00\r\nA2,PARTY-X,0.99\n";

    fn compiled(index: &str) -> Result<Box<dyn CompiledExpression>, ContractError> {
        IndexLanguage::default().compile(index, &Namespaces::new())
    }

    #[test]
    fn reads_by_record_and_field_from_one() {
        let orders = stream(ORDERS);
        let content = Content::of(&orders);
        let read = |p: &str| compiled(p)?.read(&content);
        assert_eq!(
            read("2").expect("reads"),
            Some(ScalarValue::Text("customer".into()))
        );
        assert_eq!(
            read("2/3").expect("reads"),
            Some(ScalarValue::Text("15.00".into()))
        );
        assert_eq!(
            read("3/2").expect("reads"),
            Some(ScalarValue::Text("PARTY-X".into()))
        );
        assert_eq!(read("9/1").expect("reads"), None);
        assert_eq!(read("1/9").expect("reads"), None);
        assert!(read("0/1").is_err());
        assert!(read("a/b").is_err());
        let semicolons = stream("a;b\n");
        assert_eq!(
            IndexLanguage::new(';')
                .compile("2", &Namespaces::new())
                .expect("compiles")
                .read(&Content::of(&semicolons))
                .expect("reads"),
            Some(ScalarValue::Text("b".into()))
        );
    }

    #[test]
    fn rewrites_a_field_into_a_new_stream() {
        let mut rewriting = Rewriting::of(&stream(ORDERS), StreamId::new(2));
        let mut write =
            |p: &str, v: ScalarValue| compiled(p).and_then(|c| c.write(&mut rewriting, v));
        write("2/3", ScalarValue::Decimal(16.5)).expect("writes");
        write("3/5", ScalarValue::Text("late".into())).expect("appends");
        write("1/1", ScalarValue::Null).expect("empties");
        assert!(write("9/1", ScalarValue::Null).is_err());
        assert!(write("1/1", ScalarValue::Text("a,b".into())).is_err());
        assert!(write("1/1", ScalarValue::Binary(vec![1])).is_err());
        let out = rewriting.finish().expect("finishes");
        assert_eq!(out.id(), StreamId::new(2));
        let text = std::str::from_utf8(out.load().expect("held")).expect("text");
        assert_eq!(
            text,
            ",customer,total\nA1,EXAMPLE,16.5\r\nA2,PARTY-X,0.99,,late\n"
        );
    }
}
