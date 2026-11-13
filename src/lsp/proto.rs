//! JSON-RPC över stdio med `Content-Length`-ramar, en läsartråd per server.
//!
//! [`Client`] äger serverprocessen. Svar hämtas icke-blockerande med [`Request::poll`] (gränssnittet
//! frågar varje bild), ett begäran som släpps avbryts med `$/cancelRequest`, och en begäran som tagit
//! för lång tid ger ett tidsgränsfel. Serverns egna förfrågningar (`window/workDoneProgress/create`,
//! `workspace/configuration` m.fl.) besvaras direkt i läsartråden; notiser går vidare som [`Event`].

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Anropas från läsartråden när något hänt, så att gränssnittet ritar om.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Serverns felsvar.
#[derive(Clone, Debug)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

/// Något som servern skickat och som inte är svar på en begäran.
#[derive(Debug)]
pub enum Event {
    Notification { method: String, params: Value },
    /// Servern har stängt sin utström (avslutad eller krascha).
    Exited,
}

// ------------------------------------------------------------------ ramar

/// Skriver ett meddelande med `Content-Length`-huvud.
pub fn write_message(w: &mut (impl Write + ?Sized), msg: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(msg).map_err(io::Error::other)?;
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
    w.flush()
}

/// Läser nästa meddelande. `Ok(None)` när strömmen tagit slut. Okända huvuden hoppas över.
pub fn read_message(r: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut len: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        if r.read_line(&mut line)? == 0 {
            return if len.is_some() { Err(io::ErrorKind::UnexpectedEof.into()) } else { Ok(None) };
        }
        let l = line.trim_end_matches(['\r', '\n']);
        if l.is_empty() {
            if len.is_some() {
                break;
            }
            continue;
        }
        if let Some((k, v)) = l.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
        {
            len = Some(v.trim().parse().map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "ogiltig Content-Length"))?);
        }
    }
    let len = len.unwrap_or(0);
    // Tak så att ett trasigt huvud inte ger gigantisk allokering.
    if len > 256 << 20 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "för stort meddelande"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    serde_json::from_slice(&buf).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

// ------------------------------------------------------------------ klient

type Reply = Result<Value, RpcError>;

struct Shared {
    writer: Mutex<Box<dyn Write + Send>>,
    pending: Mutex<HashMap<i64, Sender<Reply>>>,
    alive: AtomicBool,
}

impl Shared {
    fn send(&self, msg: &Value) -> bool {
        let Ok(mut w) = self.writer.lock() else { return false };
        write_message(&mut **w, msg).is_ok()
    }
}

pub struct Client {
    shared: Arc<Shared>,
    next_id: AtomicI64,
    events: Receiver<Event>,
    child: Option<Child>,
}

/// Ett pågående anrop. Släpps det innan svaret kommit avbryts det hos servern.
pub struct Request {
    id: i64,
    rx: Receiver<Reply>,
    shared: Arc<Shared>,
    deadline: Instant,
    done: bool,
}

#[derive(Debug)]
pub enum Poll {
    Pending,
    Ready(Result<Value, RpcError>),
    /// Tidsgränsen passerade; anropet är avbrutet.
    Timeout,
}

impl Request {
    pub fn poll(&mut self) -> Poll {
        if self.done {
            return Poll::Pending;
        }
        match self.rx.try_recv() {
            Ok(r) => {
                self.done = true;
                Poll::Ready(r)
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.done = true;
                Poll::Ready(Err(RpcError { code: -32099, message: "servern har avslutats".into() }))
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                if Instant::now() >= self.deadline {
                    self.cancel();
                    Poll::Timeout
                } else {
                    Poll::Pending
                }
            }
        }
    }

    /// Avbryter anropet: tar bort väntan och ber servern sluta.
    pub fn cancel(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        if let Ok(mut p) = self.shared.pending.lock() {
            p.remove(&self.id);
        }
        self.shared.send(&json!({"jsonrpc": "2.0", "method": "$/cancelRequest", "params": {"id": self.id}}));
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl Client {
    /// Startar servern i `cwd` och börjar läsa dess utdata.
    pub fn spawn(cmd: &[String], cwd: &Path, wake: Waker) -> io::Result<Client> {
        let (prog, args) = cmd.split_first().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "tomt kommando"))?;
        let mut c = Command::new(prog);
        c.args(args).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = c.spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| io::Error::other("ingen stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| io::Error::other("ingen stdout"))?;
        Ok(Self::from_streams(Box::new(stdout), Box::new(stdin), Some(child), wake))
    }

    /// Klient över godtyckliga strömmar (testas utan process).
    pub fn from_streams(reader: Box<dyn Read + Send>, writer: Box<dyn Write + Send>, child: Option<Child>, wake: Waker) -> Client {
        let shared = Arc::new(Shared { writer: Mutex::new(writer), pending: Mutex::new(HashMap::new()), alive: AtomicBool::new(true) });
        let (tx, rx) = channel();
        let sh = shared.clone();
        std::thread::Builder::new()
            .name("lsp-reader".into())
            .spawn(move || {
                let mut r = BufReader::new(reader);
                while let Ok(Some(msg)) = read_message(&mut r) {
                    dispatch(&sh, &tx, msg);
                    wake();
                }
                sh.alive.store(false, Ordering::SeqCst);
                // Släpper väntande anrop så att de svarar med fel.
                if let Ok(mut p) = sh.pending.lock() {
                    p.clear();
                }
                let _ = tx.send(Event::Exited);
                wake();
            })
            .ok();
        Client { shared, next_id: AtomicI64::new(1), events: rx, child }
    }

    pub fn alive(&self) -> bool {
        self.shared.alive.load(Ordering::SeqCst)
    }

    pub fn request(&self, method: &str, params: Value, timeout: Duration) -> Request {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = channel();
        if let Ok(mut p) = self.shared.pending.lock() {
            p.insert(id, tx);
        }
        let ok = self.shared.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        if !ok && let Ok(mut p) = self.shared.pending.lock() {
            p.remove(&id); // sändaren släpps: `poll` ger felet direkt
        }
        Request { id, rx, shared: self.shared.clone(), deadline: Instant::now() + timeout, done: false }
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.shared.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    /// Nästa händelse från servern, om någon väntar.
    pub fn next_event(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }

    /// Avslutar servern ordnat (`shutdown`, `exit`) och dödar processen om den dröjer.
    pub fn shutdown(&mut self) {
        if self.alive() {
            let mut req = self.request("shutdown", Value::Null, Duration::from_secs(2));
            let until = Instant::now() + Duration::from_millis(1500);
            while matches!(req.poll(), Poll::Pending) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            self.notify("exit", Value::Null);
        }
        if let Some(mut child) = self.child.take() {
            let until = Instant::now() + Duration::from_millis(800);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(10)),
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                }
            }
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // Sista utväg om `shutdown` inte anropats: lämna inga processer efter oss.
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Fördelar ett inkommande meddelande: svar till väntande anrop, förfrågningar besvaras, notiser vidare.
fn dispatch(sh: &Shared, events: &Sender<Event>, msg: Value) {
    let method = msg.get("method").and_then(Value::as_str);
    let id = msg.get("id");
    match (method, id) {
        (Some(m), Some(id)) => {
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            let reply = match answer_server_request(m, &params) {
                Ok(v) => json!({"jsonrpc": "2.0", "id": id, "result": v}),
                Err(()) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not found"}}),
            };
            sh.send(&reply);
        }
        (Some(m), None) => {
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            let _ = events.send(Event::Notification { method: m.to_owned(), params });
        }
        (None, Some(id)) => {
            let Some(id) = id.as_i64() else { return };
            let tx = sh.pending.lock().ok().and_then(|mut p| p.remove(&id));
            if let Some(tx) = tx {
                let reply = match msg.get("error") {
                    Some(e) => Err(RpcError {
                        code: e.get("code").and_then(Value::as_i64).unwrap_or(0),
                        message: e.get("message").and_then(Value::as_str).unwrap_or("").to_owned(),
                    }),
                    None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = tx.send(reply);
            }
        }
        (None, None) => {}
    }
}

/// Svar på serverns förfrågningar. `Err` = metoden stöds inte.
fn answer_server_request(method: &str, params: &Value) -> Result<Value, ()> {
    match method {
        "window/workDoneProgress/create" | "client/registerCapability" | "client/unregisterCapability" | "window/showMessageRequest" => Ok(Value::Null),
        // En `null` per begärd inställning: servern använder sina standardvärden.
        "workspace/configuration" => {
            let n = params.get("items").and_then(Value::as_array).map_or(0, Vec::len);
            Ok(Value::Array(vec![Value::Null; n]))
        }
        "workspace/workspaceFolders" => Ok(Value::Null),
        "workspace/applyEdit" => Ok(json!({"applied": false, "failureReason": "stöds inte"})),
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn frame_roundtrip() {
        let mut buf = Vec::new();
        write_message(&mut buf, &json!({"a": "åäö😀"})).unwrap();
        let s = String::from_utf8(buf.clone()).unwrap();
        // Längden är i byte, inte tecken.
        assert!(s.starts_with(&format!("Content-Length: {}\r\n\r\n", "{\"a\":\"åäö😀\"}".len())));
        let mut r = Cursor::new(buf);
        assert_eq!(read_message(&mut r).unwrap(), Some(json!({"a": "åäö😀"})));
        assert_eq!(read_message(&mut r).unwrap(), None);
    }

    #[test]
    fn reads_two_messages_and_extra_headers() {
        let raw = "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\ncontent-length: 7\r\n\r\n{\"x\":1}Content-Length: 4\r\n\r\nnull";
        let mut r = Cursor::new(raw.as_bytes().to_vec());
        assert_eq!(read_message(&mut r).unwrap(), Some(json!({"x": 1})));
        assert_eq!(read_message(&mut r).unwrap(), Some(Value::Null));
        assert_eq!(read_message(&mut r).unwrap(), None);
    }

    #[test]
    fn malformed_frames_are_errors() {
        let mut r = Cursor::new(b"Content-Length: abc\r\n\r\n".to_vec());
        assert!(read_message(&mut r).is_err());
        let mut r = Cursor::new(b"Content-Length: 10\r\n\r\nshort".to_vec());
        assert!(read_message(&mut r).is_err());
        let mut r = Cursor::new(b"Content-Length: 3\r\n\r\n{x}".to_vec());
        assert!(read_message(&mut r).is_err());
        let mut r = Cursor::new(b"Content-Length: 5\r\n".to_vec());
        assert!(read_message(&mut r).is_err());
    }

    /// En låtsasserver i en tråd: svarar på `echo`, ignorerar `slow` och skickar en notis.
    fn fake_client() -> (Client, Arc<Mutex<Vec<Value>>>) {
        let (c2s_r, c2s_w) = std::io::pipe().unwrap();
        let (s2c_r, s2c_w) = std::io::pipe().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        std::thread::spawn(move || {
            let mut r = BufReader::new(c2s_r);
            let mut w = s2c_w;
            // Först en egen förfrågan till klienten och en notis.
            write_message(&mut w, &json!({"jsonrpc": "2.0", "id": 77, "method": "workspace/configuration", "params": {"items": [{}, {}]}})).unwrap();
            write_message(&mut w, &json!({"jsonrpc": "2.0", "method": "hello", "params": {"n": 1}})).unwrap();
            while let Ok(Some(msg)) = read_message(&mut r) {
                seen2.lock().unwrap().push(msg.clone());
                match msg.get("method").and_then(Value::as_str) {
                    Some("echo") => {
                        write_message(&mut w, &json!({"jsonrpc": "2.0", "id": msg["id"], "result": msg["params"]})).unwrap()
                    }
                    Some("fail") => write_message(
                        &mut w,
                        &json!({"jsonrpc": "2.0", "id": msg["id"], "error": {"code": -32602, "message": "nej"}}),
                    )
                    .unwrap(),
                    _ => {}
                }
            }
        });
        let woken = Arc::new(AtomicBool::new(false));
        let w2 = woken.clone();
        (Client::from_streams(Box::new(s2c_r), Box::new(c2s_w), None, Arc::new(move || w2.store(true, Ordering::SeqCst))), seen)
    }

    fn wait<T>(mut f: impl FnMut() -> Option<T>) -> T {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(v) = f() {
                return v;
            }
            assert!(Instant::now() < until, "tidsgräns i testet");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn request_response_notification_and_server_request() {
        let (c, seen) = fake_client();
        let mut req = c.request("echo", json!({"v": 42}), Duration::from_secs(5));
        let reply = wait(|| match req.poll() {
            Poll::Ready(r) => Some(r),
            _ => None,
        });
        assert_eq!(reply.unwrap(), json!({"v": 42}));
        let mut bad = c.request("fail", Value::Null, Duration::from_secs(5));
        let e = wait(|| match bad.poll() {
            Poll::Ready(r) => Some(r),
            _ => None,
        });
        let e = e.unwrap_err();
        assert_eq!((e.code, e.message.as_str()), (-32602, "nej"));
        // Notisen kom som händelse.
        let ev = wait(|| c.next_event());
        assert!(matches!(ev, Event::Notification { ref method, .. } if method == "hello"));
        // Serverns förfrågan besvarades med en null per post.
        let answered = wait(|| seen.lock().unwrap().iter().find(|m| m.get("id") == Some(&json!(77))).cloned());
        assert_eq!(answered["result"], json!([null, null]));
    }

    #[test]
    fn timeout_and_cancel() {
        let (c, seen) = fake_client();
        let mut req = c.request("slow", Value::Null, Duration::from_millis(30));
        let id = req.id;
        std::thread::sleep(Duration::from_millis(60));
        assert!(matches!(req.poll(), Poll::Timeout));
        wait(|| seen.lock().unwrap().iter().any(|m| m["method"] == "$/cancelRequest" && m["params"]["id"] == json!(id)).then_some(()));
        // Ett släppt anrop avbryts också.
        let r2 = c.request("slow", Value::Null, Duration::from_secs(5));
        let id2 = r2.id;
        drop(r2);
        wait(|| seen.lock().unwrap().iter().any(|m| m["method"] == "$/cancelRequest" && m["params"]["id"] == json!(id2)).then_some(()));
    }

    #[test]
    fn exit_fails_pending_requests() {
        let (s2c_r, s2c_w) = std::io::pipe().unwrap();
        let (_c2s_r, c2s_w) = std::io::pipe().unwrap();
        let c = Client::from_streams(Box::new(s2c_r), Box::new(c2s_w), None, Arc::new(|| {}));
        let mut req = c.request("x", Value::Null, Duration::from_secs(5));
        drop(s2c_w); // servern stänger
        let r = wait(|| match req.poll() {
            Poll::Ready(r) => Some(r),
            _ => None,
        });
        assert!(r.is_err());
        assert!(matches!(wait(|| c.next_event()), Event::Exited));
        assert!(!c.alive());
    }
}
