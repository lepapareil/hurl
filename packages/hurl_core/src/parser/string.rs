/*
 * hurl (https://hurl.dev)
 * Copyright (C) 2020 Orange
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *          http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 *
 */
use crate::ast::*;

use super::combinators::*;
use super::error::*;
use super::expr;
use super::primitives::*;
use super::reader::{Reader, ReaderState};
use super::ParseResult;

pub type CharParser = fn(&mut Reader) -> ParseResult<(char, String)>;

pub fn unquoted_template(reader: &mut Reader) -> ParseResult<'static, Template> {
    let start = reader.state.pos.clone();

    match reader.peek() {
        Some(' ') | Some('\t') | Some('\n') | Some('#') | None => {
            return Ok(Template {
                quotes: false,
                elements: vec![],
                source_info: SourceInfo {
                    start: start.clone(),
                    end: start,
                },
            })
        }
        _ => {}
    }

    let quotes = false;
    // TODO pas escape vector =>  expected fn pointer, found closure
    let mut elements = zero_or_more(
        |reader1| template_element(|reader2| any_char(vec!['#'], reader2), reader1),
        reader,
    )?;

    // check trailing space
    if let Some(TemplateElement::String { value, encoded }) = elements.last() {
        let keep = encoded
            .trim_end_matches(|c: char| c == ' ' || c == '\t')
            .len();
        let trailing_space = encoded.len() - keep;
        if trailing_space > 0 {
            let value = value.as_str()[..value.len() - trailing_space].to_string();
            let encoded = encoded.as_str()[..encoded.len() - trailing_space].to_string();
            let new_element = TemplateElement::String { value, encoded };
            elements.pop();
            elements.push(new_element);
            let cursor = reader.state.cursor - trailing_space;
            let line = reader.state.pos.line;
            let column = reader.state.pos.column - trailing_space;
            reader.state = ReaderState {
                cursor,
                pos: Pos { line, column },
            };
        }
    }

    Ok(Template {
        quotes,
        elements,
        source_info: SourceInfo {
            start,
            end: reader.state.pos.clone(),
        },
    })
}

pub fn unquoted_string_key(reader: &mut Reader) -> ParseResult<'static, EncodedString> {
    let start = reader.state.pos.clone();

    let quotes = false;
    let mut value = "".to_string();
    let mut encoded = "".to_string();
    loop {
        let save = reader.state.clone();
        match escape_char(reader) {
            Ok(c) => {
                value.push(c);
                encoded.push_str(reader.from(save.cursor).as_str())
            }
            Err(e) => {
                if e.recoverable {
                    reader.state = save.clone();
                    match reader.read() {
                        None => break,
                        Some(c) => {
                            if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' {
                                value.push(c);
                                encoded.push_str(reader.from(save.cursor).as_str())
                            } else {
                                reader.state = save;
                                break;
                            }
                        }
                    }
                } else {
                    return Err(e);
                }
            }
        }
    }

    // check nonempty
    if value.is_empty() {
        return Err(Error {
            pos: start,
            recoverable: true,
            inner: ParseError::Expecting {
                value: "key string".to_string(),
            },
        });
    }

    let end = reader.state.pos.clone();
    let source_info = SourceInfo { start, end };
    Ok(EncodedString {
        quotes,
        encoded,
        value,
        source_info,
    })
}

// todo should return an EncodedString
// (decoding escape sequence)
pub fn quoted_string(reader: &mut Reader) -> ParseResult<'static, String> {
    literal("\"", reader)?;
    let s = reader.read_while(|c| *c != '"');
    literal("\"", reader)?;
    Ok(s)
}

pub fn quoted_template(reader: &mut Reader) -> ParseResult<'static, Template> {
    let quotes = true;
    let start = reader.state.clone().pos;
    literal("\"", reader)?;
    if reader.try_literal("\"") {
        let end = reader.state.pos.clone();
        return Ok(Template {
            quotes,
            elements: vec![],
            source_info: SourceInfo { start, end },
        });
    }

    let mut elements = vec![];
    loop {
        match template_element_expression(reader) {
            Err(e) => {
                if e.recoverable {
                    match template_element_string(|reader1| any_char(vec!['"'], reader1), reader) {
                        Err(e) => {
                            if e.recoverable {
                                break;
                            } else {
                                return Err(e);
                            }
                        }
                        Ok(element) => elements.push(element),
                    }
                } else {
                    return Err(e);
                }
            }
            Ok(element) => elements.push(element),
        }
    }

    literal("\"", reader)?;
    let end = reader.state.pos.clone();
    Ok(Template {
        quotes,
        elements,
        source_info: SourceInfo { start, end },
    })
}

fn template_element(
    char_parser: CharParser,
    reader: &mut Reader,
) -> ParseResult<'static, TemplateElement> {
    match template_element_expression(reader) {
        Err(e) => {
            if e.recoverable {
                template_element_string(char_parser, reader)
            } else {
                Err(e)
            }
        }
        r => r,
    }
}

fn template_element_string(
    char_parser: CharParser,
    reader: &mut Reader,
) -> ParseResult<'static, TemplateElement> {
    let start = reader.state.clone();
    let mut value = "".to_string();
    let mut encoded = "".to_string();

    let mut bracket = false;
    let mut end_pos = start.clone();
    loop {
        match char_parser(reader) {
            Err(e) => {
                if e.recoverable {
                    break;
                } else {
                    return Err(e);
                }
            }
            Ok((c, s)) => {
                if s == "{" && bracket {
                    break;
                } else if s == "{" && !bracket {
                    bracket = true;
                } else if bracket {
                    bracket = false;
                    value.push('{');
                    encoded.push('{');
                    value.push(c);
                    encoded.push_str(s.as_str());
                } else {
                    end_pos = reader.state.clone();
                    value.push(c);
                    encoded.push_str(s.as_str());
                }
            }
        }
    }
    reader.state = end_pos;

    if value.is_empty() {
        Err(Error {
            pos: start.pos,
            recoverable: true,
            inner: ParseError::Expecting {
                value: "string".to_string(),
            },
        })
    } else {
        Ok(TemplateElement::String { value, encoded })
    }
}

fn template_element_expression(reader: &mut Reader) -> ParseResult<'static, TemplateElement> {
    let value = expr::parse(reader)?;
    Ok(TemplateElement::Expression(value))
}

fn any_char(except: Vec<char>, reader: &mut Reader) -> ParseResult<'static, (char, String)> {
    let start = reader.state.clone();
    match escape_char(reader) {
        Ok(c) => Ok((c, reader.from(start.cursor))),
        Err(e) => {
            if e.recoverable {
                reader.state = start.clone();
                match reader.read() {
                    None => Err(Error {
                        pos: start.pos,
                        recoverable: true,
                        inner: ParseError::Expecting {
                            value: "char".to_string(),
                        },
                    }),
                    Some(c) => {
                        if except.contains(&c)
                            || vec!['\\', '\x08', '\n', '\x0c', '\r', '\t'].contains(&c)
                        {
                            Err(Error {
                                pos: start.pos,
                                recoverable: true,
                                inner: ParseError::Expecting {
                                    value: "char".to_string(),
                                },
                            })
                        } else {
                            Ok((c, reader.from(start.cursor)))
                        }
                    }
                }
            } else {
                Err(e)
            }
        }
    }
}

fn escape_char(reader: &mut Reader) -> ParseResult<'static, char> {
    try_literal("\\", reader)?;
    let start = reader.state.clone();
    match reader.read() {
        // Some('#') => Ok('#'),
        Some('"') => Ok('"'),
        Some('\\') => Ok('\\'),
        Some('/') => Ok('/'),
        Some('b') => Ok('\x08'),
        Some('n') => Ok('\n'),
        Some('f') => Ok('\x0c'),
        Some('r') => Ok('\r'),
        Some('t') => Ok('\t'),
        Some('u') => unicode(reader),
        _ => Err(Error {
            pos: start.pos,
            recoverable: false,
            inner: ParseError::EscapeChar {},
        }),
    }
}

fn unicode(reader: &mut Reader) -> ParseResult<'static, char> {
    literal("{", reader)?;
    let v = hex_value(reader)?;
    let c = match std::char::from_u32(v) {
        None => {
            return Err(Error {
                pos: reader.clone().state.pos,
                recoverable: false,
                inner: ParseError::Unicode {},
            })
        }
        Some(c) => c,
    };
    literal("}", reader)?;
    Ok(c)
}

fn hex_value(reader: &mut Reader) -> ParseResult<'static, u32> {
    let mut digits = one_or_more(|p1| hex_digit(p1), reader)?;
    let mut v = 0;
    let mut weight = 1;
    digits.reverse();
    for d in digits.iter() {
        v += weight * d;
        weight *= 16;
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unquoted_template() {
        let mut reader = Reader::init("");
        assert_eq!(
            unquoted_template(&mut reader).unwrap(),
            Template {
                quotes: false,
                elements: vec![],
                source_info: SourceInfo::init(1, 1, 1, 1),
            }
        );
        assert_eq!(reader.state.cursor, 0);

        let mut reader = Reader::init("a\\u{23}");
        assert_eq!(
            unquoted_template(&mut reader).unwrap(),
            Template {
                quotes: false,
                elements: vec![TemplateElement::String {
                    value: "a#".to_string(),
                    encoded: "a\\u{23}".to_string()
                }],
                source_info: SourceInfo::init(1, 1, 1, 8),
            }
        );
        assert_eq!(reader.state.cursor, 7);

        let mut reader = Reader::init("hello\\u{20}{{name}}!");
        assert_eq!(
            unquoted_template(&mut reader).unwrap(),
            Template {
                quotes: false,
                elements: vec![
                    TemplateElement::String {
                        value: "hello ".to_string(),
                        encoded: "hello\\u{20}".to_string()
                    },
                    TemplateElement::Expression(Expr {
                        space0: Whitespace {
                            value: "".to_string(),
                            source_info: SourceInfo::init(1, 14, 1, 14)
                        },
                        variable: Variable {
                            name: "name".to_string(),
                            source_info: SourceInfo::init(1, 14, 1, 18)
                        },
                        space1: Whitespace {
                            value: "".to_string(),
                            source_info: SourceInfo::init(1, 18, 1, 18)
                        },
                    }),
                    TemplateElement::String {
                        value: "!".to_string(),
                        encoded: "!".to_string()
                    },
                ],
                source_info: SourceInfo::init(1, 1, 1, 21),
            }
        );
        assert_eq!(reader.state.cursor, 20);

        let mut reader = Reader::init("hello\n");
        assert_eq!(
            unquoted_template(&mut reader).unwrap(),
            Template {
                quotes: false,
                elements: vec![TemplateElement::String {
                    value: "hello".to_string(),
                    encoded: "hello".to_string()
                },],
                source_info: SourceInfo::init(1, 1, 1, 6),
            }
        );
        assert_eq!(reader.state.cursor, 5);
    }

    #[test]
    fn test_unquoted_template_trailing_space() {
        let mut reader = Reader::init("hello # comment");
        assert_eq!(
            unquoted_template(&mut reader).unwrap(),
            Template {
                quotes: false,
                elements: vec![TemplateElement::String {
                    value: "hello".to_string(),
                    encoded: "hello".to_string()
                },],
                source_info: SourceInfo::init(1, 1, 1, 6),
            }
        );
        assert_eq!(reader.state.cursor, 5);
        assert_eq!(reader.state.pos, Pos { line: 1, column: 6 });
    }

    #[test]
    fn test_unquoted_template_empty() {
        let mut reader = Reader::init(" hi");
        assert_eq!(
            unquoted_template(&mut reader).unwrap(),
            Template {
                quotes: false,
                elements: vec![],
                source_info: SourceInfo::init(1, 1, 1, 1),
            }
        );

        assert_eq!(reader.state.cursor, 0);
    }

    #[test]
    fn test_unquoted_key() {
        let mut reader = Reader::init("key");
        assert_eq!(
            unquoted_string_key(&mut reader).unwrap(),
            EncodedString {
                value: "key".to_string(),
                encoded: "key".to_string(),
                quotes: false,
                source_info: SourceInfo::init(1, 1, 1, 4),
            }
        );
        assert_eq!(reader.state.cursor, 3);

        let mut reader = Reader::init("key\\u{20}\\u{3a} :");
        assert_eq!(
            unquoted_string_key(&mut reader).unwrap(),
            EncodedString {
                value: "key :".to_string(),
                encoded: "key\\u{20}\\u{3a}".to_string(),
                quotes: false,
                source_info: SourceInfo::init(1, 1, 1, 16),
            }
        );
        assert_eq!(reader.state.cursor, 15);
    }

    #[test]
    fn test_unquoted_key_error() {
        let mut reader = Reader::init("");
        let error = unquoted_string_key(&mut reader).err().unwrap();
        assert_eq!(error.pos, Pos { line: 1, column: 1 });
        assert_eq!(
            error.inner,
            ParseError::Expecting {
                value: "key string".to_string()
            }
        );

        let mut reader = Reader::init("\\l");
        let error = unquoted_string_key(&mut reader).err().unwrap();
        assert_eq!(error.pos, Pos { line: 1, column: 2 });
        assert_eq!(error.inner, ParseError::EscapeChar {});
    }

    #[test]
    fn test_quoted_template() {
        let mut reader = Reader::init("\"\"");
        assert_eq!(
            quoted_template(&mut reader).unwrap(),
            Template {
                quotes: true,
                elements: vec![],
                source_info: SourceInfo::init(1, 1, 1, 3),
            }
        );
        assert_eq!(reader.state.cursor, 2);

        let mut reader = Reader::init("\"a#\"");
        assert_eq!(
            quoted_template(&mut reader).unwrap(),
            Template {
                quotes: true,
                elements: vec![TemplateElement::String {
                    value: "a#".to_string(),
                    encoded: "a#".to_string()
                }],
                source_info: SourceInfo::init(1, 1, 1, 5),
            }
        );
        assert_eq!(reader.state.cursor, 4);

        let mut reader = Reader::init("\"{0}\"");
        assert_eq!(
            quoted_template(&mut reader).unwrap(),
            Template {
                quotes: true,
                elements: vec![TemplateElement::String {
                    value: "{0}".to_string(),
                    encoded: "{0}".to_string()
                }],
                source_info: SourceInfo::init(1, 1, 1, 6),
            }
        );
        assert_eq!(reader.state.cursor, 5);
    }

    #[test]
    fn test_quoted_string() {
        let mut reader = Reader::init("\"\"");
        assert_eq!(quoted_string(&mut reader).unwrap(), "");
        assert_eq!(reader.state.cursor, 2);

        let mut reader = Reader::init("\"Hello\"");
        assert_eq!(quoted_string(&mut reader).unwrap(), "Hello");
        assert_eq!(reader.state.cursor, 7);
    }

    #[test]
    fn test_template_element_unquoted_string() {
        let mut reader = Reader::init("name\\u{23}\\u{20}{{");
        assert_eq!(
            template_element_string(|reader1| any_char(vec![], reader1), &mut reader).unwrap(),
            TemplateElement::String {
                value: "name# ".to_string(),
                encoded: "name\\u{23}\\u{20}".to_string(),
            }
        );
        assert_eq!(reader.state.cursor, 16);
    }

    #[test]
    fn test_template_element_unquoted_string_single_bracket() {
        let mut reader = Reader::init("{0}");
        assert_eq!(
            template_element_string(|reader1| any_char(vec![], reader1), &mut reader).unwrap(),
            TemplateElement::String {
                value: "{0}".to_string(),
                encoded: "{0}".to_string(),
            }
        );
        assert_eq!(reader.state.cursor, 3);
    }

    #[test]
    fn test_template_element_quoted_string() {
        let mut reader = Reader::init("name#\\u{20}{{");
        assert_eq!(
            template_element_string(|reader1| any_char(vec![], reader1), &mut reader).unwrap(),
            TemplateElement::String {
                value: "name# ".to_string(),
                encoded: "name#\\u{20}".to_string(),
            }
        );
        assert_eq!(reader.state.cursor, 11);
    }

    #[test]
    fn test_template_element_expression() {
        let mut reader = Reader::init("{{name}}");
        assert_eq!(
            template_element_expression(&mut reader).unwrap(),
            TemplateElement::Expression(Expr {
                space0: Whitespace {
                    value: "".to_string(),
                    source_info: SourceInfo::init(1, 3, 1, 3)
                },
                variable: Variable {
                    name: "name".to_string(),
                    source_info: SourceInfo::init(1, 3, 1, 7)
                },
                space1: Whitespace {
                    value: "".to_string(),
                    source_info: SourceInfo::init(1, 7, 1, 7)
                },
            })
        );
    }

    #[test]
    fn test_any_char() {
        let mut reader = Reader::init("a");
        assert_eq!(
            any_char(vec![], &mut reader).unwrap(),
            ('a', "a".to_string())
        );
        assert_eq!(reader.state.cursor, 1);

        let mut reader = Reader::init(" ");
        assert_eq!(
            any_char(vec![], &mut reader).unwrap(),
            (' ', " ".to_string())
        );
        assert_eq!(reader.state.cursor, 1);

        let mut reader = Reader::init("\\t");
        assert_eq!(
            any_char(vec![], &mut reader).unwrap(),
            ('\t', "\\t".to_string())
        );
        assert_eq!(reader.state.cursor, 2);

        let mut reader = Reader::init("#");
        assert_eq!(
            any_char(vec![], &mut reader).unwrap(),
            ('#', "#".to_string())
        );
        assert_eq!(reader.state.cursor, 1);
    }

    #[test]
    fn test_any_char_error() {
        let mut reader = Reader::init("");
        let error = any_char(vec![], &mut reader).err().unwrap();
        assert_eq!(error.pos, Pos { line: 1, column: 1 });
        assert_eq!(error.recoverable, true);

        let mut reader = Reader::init("#");
        let error = any_char(vec!['#'], &mut reader).err().unwrap();
        assert_eq!(error.pos, Pos { line: 1, column: 1 });
        assert_eq!(error.recoverable, true);

        let mut reader = Reader::init("\t");
        let error = any_char(vec![], &mut reader).err().unwrap();
        assert_eq!(error.pos, Pos { line: 1, column: 1 });
        assert_eq!(error.recoverable, true);
    }

    #[test]
    fn test_escape_char() {
        let mut reader = Reader::init("\\n");
        assert_eq!(escape_char(&mut reader).unwrap(), '\n');
        assert_eq!(reader.state.cursor, 2);

        let mut reader = Reader::init("\\u{0a}");
        assert_eq!(escape_char(&mut reader).unwrap(), '\n');
        assert_eq!(reader.state.cursor, 6);

        let mut reader = Reader::init("x");
        let error = escape_char(&mut reader).err().unwrap();
        assert_eq!(error.pos, Pos { line: 1, column: 1 });
        assert_eq!(
            error.inner,
            ParseError::Expecting {
                value: "\\".to_string()
            }
        );
        assert_eq!(error.recoverable, true);
        assert_eq!(reader.state.cursor, 0);
    }

    #[test]
    fn test_unicode() {
        let mut reader = Reader::init("{000a}");
        assert_eq!(unicode(&mut reader).unwrap(), '\n');
        assert_eq!(reader.state.cursor, 6);

        let mut reader = Reader::init("{E9}");
        assert_eq!(unicode(&mut reader).unwrap(), 'é');
        assert_eq!(reader.state.cursor, 4);
    }

    #[test]
    fn test_hex_value() {
        let mut reader = Reader::init("20x");
        assert_eq!(hex_value(&mut reader).unwrap(), 32);

        let mut reader = Reader::init("x");
        let error = hex_value(&mut reader).err().unwrap();
        assert_eq!(error.pos, Pos { line: 1, column: 1 });
        assert_eq!(error.inner, ParseError::HexDigit);
        assert_eq!(error.recoverable, false);
    }
}
