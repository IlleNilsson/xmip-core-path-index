#![forbid(unsafe_code)]

//! The index path technology — a technology of `xmip-core-path`.
//!
//! Three things, because a path language is nothing without content to address:
//! [`IndexEngine`], the [`PathEngine`] for the language `index`;
//! [`DelimitedStructure`], a [`StructureReader`] over a delimited text Stream;
//! and [`DelimitedRewrite`], a [`StructureWriter`] that produces a new Stream
//! with one or more fields replaced, as ADR-0013 asks of anything that changes
//! content. Promote reads through the first two; demote writes through the
//! first and third; route and process read.
//!
//! An index is `record/field`, both counted from 1, or `field` alone for the
//! first record: `3/2` is the second field of the third record. Records are
//! lines; fields are split on the delimiter, comma unless the structure is
//! opened with another. Quoting is the CSV contract's concern, not this one's:
//! an index addresses positions, and a value is the text found there.

use contract::{
    ContractDescriptor, ContractError, ContractId, StructureReader, StructureWriter,
    StructuredValue,
};
use path::{Path, PathCost, PathEngine};
use stream::Stream;
use xcore::StreamId;

/// The `index` engine. The reader speaks indexes already, so the engine adds
/// no traversal of its own.
pub struct IndexEngine;

impl PathEngine for IndexEngine {
    fn language(&self) -> &'static str {
        "index"
    }

    fn read(
        &self,
        reader: &dyn StructureReader,
        path: &Path,
    ) -> Result<Option<StructuredValue>, ContractError> {
        reader.read(&path.expression)
    }

    fn write(
        &self,
        writer: &mut dyn StructureWriter,
        path: &Path,
        value: StructuredValue,
    ) -> Result<(), ContractError> {
        writer.write(&path.expression, value)
    }

    /// A field of the first record is at the front of the Stream; any other
    /// record is a scan to reach, never a copy.
    fn cost(&self, path: &Path) -> PathCost {
        match record_and_field(&path.expression) {
            Ok((1, _)) => PathCost::StreamPrefix,
            _ => PathCost::StreamScan,
        }
    }
}

fn descriptor() -> ContractDescriptor {
    ContractDescriptor {
        id: ContractId("csv".to_string()),
        version: "1".to_string(),
        representation: "text/csv".to_string(),
    }
}

/// `record/field` or `field`, both from 1.
fn record_and_field(expression: &str) -> Result<(usize, usize), ContractError> {
    let refuse = || ContractError {
        message: format!("{expression:?} is not record/field or field, counted from 1"),
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

fn records(stream: &Stream, delimiter: char) -> Result<Vec<Vec<String>>, ContractError> {
    let text = std::str::from_utf8(stream.bytes()).map_err(|error| ContractError {
        message: format!("not UTF-8 text: {error}"),
    })?;
    Ok(text
        .lines()
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .map(|line| line.split(delimiter).map(str::to_string).collect())
        .collect())
}

/// A delimited text Stream, read by index.
pub struct DelimitedStructure {
    descriptor: ContractDescriptor,
    records: Vec<Vec<String>>,
}

impl DelimitedStructure {
    /// Split `stream` once on `delimiter`; every read is a lookup after that.
    ///
    /// # Errors
    /// The Stream is not text.
    pub fn parse(stream: &Stream, delimiter: char) -> Result<Self, ContractError> {
        Ok(Self {
            descriptor: descriptor(),
            records: records(stream, delimiter)?,
        })
    }
}

impl StructureReader for DelimitedStructure {
    fn contract(&self) -> &ContractDescriptor {
        &self.descriptor
    }

    fn read(&self, path: &str) -> Result<Option<StructuredValue>, ContractError> {
        let (record, field) = record_and_field(path)?;
        Ok(self
            .records
            .get(record - 1)
            .and_then(|fields| fields.get(field - 1))
            .map(|text| StructuredValue::Text(text.clone())))
    }
}

/// A delimited text Stream being rewritten into a new one.
pub struct DelimitedRewrite {
    descriptor: ContractDescriptor,
    id: StreamId,
    delimiter: char,
    records: Vec<Vec<String>>,
}

impl DelimitedRewrite {
    /// Start from `stream`; the Stream `finish` produces carries `id`.
    ///
    /// # Errors
    /// The Stream is not text.
    pub fn of(stream: &Stream, delimiter: char, id: StreamId) -> Result<Self, ContractError> {
        Ok(Self {
            descriptor: descriptor(),
            id,
            delimiter,
            records: records(stream, delimiter)?,
        })
    }
}

impl StructureWriter for DelimitedRewrite {
    fn contract(&self) -> &ContractDescriptor {
        &self.descriptor
    }

    /// Replace one field. A record that does not exist is refused; a field
    /// past the end of an existing record is appended, padding with empties,
    /// because a delimited record has no declared width to violate.
    fn write(&mut self, path: &str, value: StructuredValue) -> Result<(), ContractError> {
        let (record, field) = record_and_field(path)?;
        let text = lexical(value, self.delimiter)?;
        let fields = self
            .records
            .get_mut(record - 1)
            .ok_or_else(|| ContractError {
                message: format!("record {record} does not exist"),
            })?;
        if fields.len() < field {
            fields.resize(field, String::new());
        }
        fields[field - 1] = text;
        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<Stream, ContractError> {
        let mut text = String::new();
        for fields in &self.records {
            text.push_str(&fields.join(&self.delimiter.to_string()));
            text.push('\n');
        }
        Ok(Stream::new(
            self.id,
            text.into_bytes(),
            Some(self.descriptor.representation),
        ))
    }
}

/// The text a value is written as. A value carrying the delimiter or a line
/// break would change the record count for every reader after us, so it is
/// refused rather than quoted: quoting is the CSV contract's dialect.
fn lexical(value: StructuredValue, delimiter: char) -> Result<String, ContractError> {
    let text = match value {
        StructuredValue::Null => String::new(),
        StructuredValue::Bool(flag) => flag.to_string(),
        StructuredValue::Integer(integer) => integer.to_string(),
        StructuredValue::Decimal(decimal) => decimal.to_string(),
        StructuredValue::Text(text) => text,
        StructuredValue::Binary(_) => {
            return Err(ContractError {
                message: "binary has no text form here".to_string(),
            });
        }
    };
    if text.contains(delimiter) || text.contains(['\n', '\r']) {
        return Err(ContractError {
            message: format!("{text:?} would break the record it is written into"),
        });
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use path::fixture::stream;

    const ORDERS: &str = "id,customer,total\nA1,ACME,15.00\nA2,BOLT,0.99\n";

    #[test]
    fn reads_by_record_and_field_from_one() {
        let structure = DelimitedStructure::parse(&stream(ORDERS), ',').expect("parses");
        let engine = IndexEngine;
        let read = |p: &str| engine.read(&structure, &Path::new("index", p));
        assert_eq!(
            read("2").expect("reads"),
            Some(StructuredValue::Text("customer".into()))
        );
        assert_eq!(
            read("2/3").expect("reads"),
            Some(StructuredValue::Text("15.00".into()))
        );
        assert_eq!(
            read("3/2").expect("reads"),
            Some(StructuredValue::Text("BOLT".into()))
        );
        assert_eq!(read("9/1").expect("reads"), None);
        assert_eq!(read("1/9").expect("reads"), None);
        assert!(read("0/1").is_err());
        assert!(read("a/b").is_err());
    }

    #[test]
    fn the_first_record_is_a_prefix_and_the_rest_a_scan() {
        let engine = IndexEngine;
        assert_eq!(
            engine.cost(&Path::new("index", "1/2")),
            PathCost::StreamPrefix
        );
        assert_eq!(
            engine.cost(&Path::new("index", "2")),
            PathCost::StreamPrefix
        );
        assert_eq!(
            engine.cost(&Path::new("index", "7/2")),
            PathCost::StreamScan
        );
    }

    #[test]
    fn rewrites_a_field_into_a_new_stream() {
        let mut rewrite =
            DelimitedRewrite::of(&stream(ORDERS), ',', StreamId::new(2)).expect("parses");
        let engine = IndexEngine;
        engine
            .write(
                &mut rewrite,
                &Path::new("index", "2/3"),
                StructuredValue::Decimal(16.5),
            )
            .expect("writes");
        engine
            .write(
                &mut rewrite,
                &Path::new("index", "3/5"),
                StructuredValue::Text("late".into()),
            )
            .expect("appends");
        assert!(rewrite.write("9/1", StructuredValue::Null).is_err());
        assert!(
            rewrite
                .write("1/1", StructuredValue::Text("a,b".into()))
                .is_err()
        );
        let out = Box::new(rewrite).finish().expect("finishes");
        assert_eq!(out.id(), StreamId::new(2));
        let text = std::str::from_utf8(out.bytes()).expect("text");
        assert_eq!(
            text,
            "id,customer,total\nA1,ACME,16.5\nA2,BOLT,0.99,,late\n"
        );
    }
}
