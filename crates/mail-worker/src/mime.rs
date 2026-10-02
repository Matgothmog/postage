//! The readable parts of an inbound message: what the classifier is shown.
//! Replaces `postal-mime` in `worker/src/index.ts`.

use mail_parser::{HeaderName, Message, MessageParser, PartType};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReadableMessage {
    pub subject: String,
    pub body: String,
    /// The one address in the `From:` header, which is the address DMARC
    /// evaluates. `None` unless there is exactly one `From:` header naming
    /// exactly one address: anything else is ambiguous about which domain a
    /// DMARC result was for.
    pub header_from: Option<String>,
}

/// The subject and the text of a message, preferring `text/plain` and falling
/// back to `text/html` only when the message has no plain part at all. HTML is
/// handed over as written, never converted: the classifier is told what the
/// sender sent.
///
/// A message the parser cannot find headers in reads as empty rather than as an
/// error. The raw bytes are what get held or forwarded, so a poor rendering
/// costs the classifier some context and never the message.
pub fn read_message(raw: &[u8]) -> ReadableMessage {
    let Some(message) = MessageParser::default().parse(raw) else {
        return ReadableMessage::default();
    };

    let plain: Vec<&str> = message
        .text_bodies()
        .filter(|part| matches!(part.body, PartType::Text(_)))
        .filter_map(|part| part.text_contents())
        .collect();
    let html: Vec<&str> = message
        .html_bodies()
        .filter(|part| matches!(part.body, PartType::Html(_)))
        .filter_map(|part| part.text_contents())
        .collect();

    let body = if !plain.is_empty() {
        plain.join("\n")
    } else {
        html.join("\n")
    };

    ReadableMessage {
        subject: message.subject().unwrap_or_default().to_owned(),
        body,
        header_from: sole_from_address(&message),
    }
}

fn sole_from_address(message: &Message<'_>) -> Option<String> {
    let mut headers = message.header_values(HeaderName::From);
    let header = headers.next()?;
    if headers.next().is_some() {
        return None;
    }
    let mut addresses = header.as_address()?.iter();
    let address = addresses.next()?.address.as_deref()?;
    if addresses.next().is_some() {
        return None;
    }
    Some(address.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_and_plain_text_are_read() {
        let raw = "From: a@b.c\r\nSubject: A question\r\n\r\nIs this thing on?";
        let read = read_message(raw.as_bytes());
        assert_eq!(read.subject, "A question");
        assert!(read.body.contains("Is this thing on?"));
    }

    #[test]
    fn an_html_only_message_falls_back_to_its_html() {
        let raw = "From: a@b.c\r\nSubject: s\r\nContent-Type: text/html\r\n\r\n<p>hello</p>";
        assert_eq!(read_message(raw.as_bytes()).body, "<p>hello</p>");
    }

    #[test]
    fn plain_text_wins_over_html_in_an_alternative() {
        let raw = "From: a@b.c\r\nSubject: s\r\nContent-Type: multipart/alternative; boundary=XX\r\n\r\n\
            --XX\r\nContent-Type: text/plain\r\n\r\nplain version\r\n\
            --XX\r\nContent-Type: text/html\r\n\r\n<p>html version</p>\r\n--XX--\r\n";
        assert_eq!(read_message(raw.as_bytes()).body.trim(), "plain version");
    }

    #[test]
    fn the_from_header_address_is_read_bare() {
        let raw = "From: \"Anna\" <anna@acme.example>\r\nSubject: s\r\n\r\nhi";
        let read = read_message(raw.as_bytes());
        assert_eq!(read.header_from.as_deref(), Some("anna@acme.example"));
    }

    #[test]
    fn a_from_header_naming_two_addresses_reads_as_none() {
        let raw = "From: a@one.example, b@two.example\r\nSubject: s\r\n\r\nhi";
        assert_eq!(read_message(raw.as_bytes()).header_from, None);
    }

    #[test]
    fn two_from_headers_read_as_none() {
        let raw = "From: a@one.example\r\nFrom: b@two.example\r\nSubject: s\r\n\r\nhi";
        assert_eq!(read_message(raw.as_bytes()).header_from, None);
    }

    #[test]
    fn a_message_without_a_from_header_reads_as_none() {
        let raw = "Subject: s\r\n\r\nhi";
        assert_eq!(read_message(raw.as_bytes()).header_from, None);
    }

    #[test]
    fn bytes_with_no_headers_read_as_empty() {
        assert_eq!(read_message(b""), ReadableMessage::default());
    }
}
