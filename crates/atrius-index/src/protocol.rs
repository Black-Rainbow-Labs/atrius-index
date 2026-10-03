//! The wire format between a caller and an index host: one JSON object a line, one request a
//! connection, on a loopback port.
//!
//! ```text
//! request  {"token":"<32 hex>","command":"find","arguments":{"query":"retry"}}
//! reply    {"ok":true,"command":"find","result":{...,"text":"..."}}
//! refusal  {"ok":false,"command":"find","error":{"code":"usage","message":"..."}}
//! ```
//!
//! A command may carry the prefix `search.`, which is how Unluminous names the same commands, so a
//! host started by either program answers the other.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde_json::{json, Map, Value};

/// The codes a refusal carries.
pub mod code {
    /// No index host is running for the folder, and none could be started.
    pub const NOT_RUNNING: &str = "not-running";
    /// There is no command by that name.
    pub const UNKNOWN_COMMAND: &str = "unknown-command";
    /// The command exists but was given the wrong thing.
    pub const USAGE: &str = "usage";
    /// The token was missing or wrong.
    pub const REFUSED: &str = "refused";
    /// What was asked for is not there: a folder, a file, a line.
    pub const NOT_FOUND: &str = "not-found";
    /// It was tried and it did not work. The message says what went wrong.
    pub const FAILED: &str = "failed";
    /// It was still going when the time ran out.
    pub const TIMED_OUT: &str = "timed-out";
}

/// One request to a host.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    /// The host's token, from its host file.
    pub token: String,
    /// The command, such as `find`.
    pub command: String,
    /// The command's named arguments.
    pub arguments: Map<String, Value>,
}

impl Request {
    /// A request with its token, command and arguments.
    ///
    /// @param token - the host's token
    /// @param command - the command
    /// @param arguments - the arguments
    pub fn new(token: &str, command: &str, arguments: Map<String, Value>) -> Self {
        Request { token: token.to_owned(), command: command.to_owned(), arguments }
    }

    /// The request as the JSON object sent on the wire.
    pub fn to_json(&self) -> Value {
        json!({ "token": self.token, "command": self.command, "arguments": self.arguments })
    }

    /// Reads a request from its JSON object. `None` when it is not one.
    ///
    /// @param value - the object
    pub fn from_json(value: &Value) -> Option<Self> {
        Some(Request {
            token: value.get("token")?.as_str()?.to_owned(),
            command: value.get("command")?.as_str()?.to_owned(),
            arguments: value.get("arguments").and_then(Value::as_object).cloned().unwrap_or_default(),
        })
    }
}

/// One reply from a host.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    /// Whether the command was carried out.
    pub ok: bool,
    /// The command the reply is for.
    pub command: String,
    /// The answer, when it was carried out.
    pub result: Value,
    /// The refusal's code, when it was not.
    pub code: String,
    /// What to tell the caller, when it was not.
    pub message: String,
}

impl Reply {
    /// A reply saying the command was carried out.
    ///
    /// @param command - the command
    /// @param result - the answer
    pub fn done(command: &str, result: Value) -> Self {
        Reply {
            ok: true,
            command: command.to_owned(),
            result,
            code: String::new(),
            message: String::new(),
        }
    }

    /// A reply saying the command was refused.
    ///
    /// @param command - the command
    /// @param code - one of [`code`]
    /// @param message - what went wrong
    pub fn failed(command: &str, code: &str, message: impl Into<String>) -> Self {
        Reply {
            ok: false,
            command: command.to_owned(),
            result: Value::Null,
            code: code.to_owned(),
            message: message.into(),
        }
    }

    /// The reply as the JSON object sent on the wire and printed by `--json`.
    pub fn to_json(&self) -> Value {
        if self.ok {
            json!({ "ok": true, "command": self.command, "result": self.result })
        } else {
            json!({
                "ok": false,
                "command": self.command,
                "error": { "code": self.code, "message": self.message },
            })
        }
    }

    /// Reads a reply from its JSON object. `None` when it is not one.
    ///
    /// @param value - the object
    pub fn from_json(value: &Value) -> Option<Self> {
        let ok = value.get("ok")?.as_bool()?;
        let command = value.get("command").and_then(Value::as_str).unwrap_or_default();
        if ok {
            return Some(Reply::done(command, value.get("result").cloned().unwrap_or(Value::Null)));
        }
        let error = value.get("error")?;
        Some(Reply::failed(
            command,
            error.get("code").and_then(Value::as_str).unwrap_or(code::FAILED),
            error.get("message").and_then(Value::as_str).unwrap_or_default(),
        ))
    }

    /// What a person reads: the answer's `text` when it was carried out, the message when it was not.
    pub fn text(&self) -> String {
        if self.ok {
            self.result.get("text").and_then(Value::as_str).unwrap_or_default().to_owned()
        } else {
            format!("{}: {}", self.code, self.message)
        }
    }
}

/// Sends one request to a host on this machine and reads the one reply.
///
/// @param port - the host's loopback port
/// @param request - the request
/// @param timeout - how long to wait for the reply
pub fn ask(port: u16, request: &Request, timeout: Duration) -> std::io::Result<Reply> {
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let stream = TcpStream::connect_timeout(&address, timeout.min(Duration::from_secs(5)))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let mut writer = stream.try_clone()?;
    writeln!(writer, "{}", request.to_json())?;
    writer.flush()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let value: Value = serde_json::from_str(line.trim())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Reply::from_json(&value)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "not a reply"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_and_a_reply_read_back_as_they_were_written() {
        let mut arguments = Map::new();
        arguments.insert("query".into(), Value::String("retry".into()));
        let request = Request::new("abc", "find", arguments);
        assert_eq!(Request::from_json(&request.to_json()), Some(request));
        let done = Reply::done("find", json!({ "text": "a.rs:1: retry\n" }));
        assert_eq!(Reply::from_json(&done.to_json()), Some(done.clone()));
        assert_eq!(done.text(), "a.rs:1: retry\n");
        let refused = Reply::failed("find", code::USAGE, "no query");
        assert_eq!(Reply::from_json(&refused.to_json()), Some(refused.clone()));
        assert_eq!(refused.text(), "usage: no query");
    }
}
