use std::future::Future;
use std::io::{self, Read};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use nobs_core::device::{Credentials, Error, Terminal, Transport, ANSWER_LIMIT};
use nobs_core::runner::Output;
use russh::keys::PublicKeyOrCertificate;
use russh::{client, Channel, ChannelMsg, ChannelReadHalf, ChannelWriteHalf};

const SIZE_CHECK: Duration = Duration::from_millis(250);

pub struct Ssh;

impl Transport for Ssh {
    fn run(&self, host: &str, credentials: &Credentials, command: &str) -> Result<Output, Error> {
        run_at(host, 22, credentials, command, ANSWER_LIMIT)
    }

    fn shell(
        &self,
        host: &str,
        credentials: &Credentials,
        terminal: &mut dyn Terminal,
    ) -> Result<i32, Error> {
        shell_at(host, 22, credentials, terminal, ANSWER_LIMIT)
    }
}

struct AcceptAnyKey;

impl client::Handler for AcceptAnyKey {
    type Error = russh::Error;

    async fn check_server_key(&mut self, _: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

fn run_at(
    host: &str,
    port: u16,
    credentials: &Credentials,
    command: &str,
    limit: Duration,
) -> Result<Output, Error> {
    block_on(session(host, port, credentials, command, limit))
}

fn shell_at(
    host: &str,
    port: u16,
    credentials: &Credentials,
    terminal: &mut dyn Terminal,
    limit: Duration,
) -> Result<i32, Error> {
    block_on(live_shell(host, port, credentials, terminal, limit))
}

fn block_on<T>(work: impl Future<Output = Result<T, Error>>) -> Result<T, Error> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| Error::Library(error.to_string()))?
        .block_on(work)
}

async fn open(
    host: &str,
    port: u16,
    credentials: &Credentials,
    limit: Duration,
) -> Result<(client::Handle<AcceptAnyKey>, Channel<client::Msg>), Error> {
    let config = Arc::new(client::Config::default());
    let connection = client::connect(config, (host, port), AcceptAnyKey);
    let mut handle = within(limit, connection, host).await?;
    let password = credentials.password.expose();
    let login = handle.authenticate_password(credentials.user.as_str(), password);
    if !within(limit, login, host).await?.success() {
        return Err(Error::AuthenticationFailed);
    }
    let channel = within(limit, handle.channel_open_session(), host).await?;
    Ok((handle, channel))
}

async fn session(
    host: &str,
    port: u16,
    credentials: &Credentials,
    command: &str,
    limit: Duration,
) -> Result<Output, Error> {
    let (_handle, mut channel) = open(host, port, credentials, limit).await?;
    within(limit, channel.exec(true, command), host).await?;
    answer(&mut channel, limit, host).await
}

async fn live_shell(
    host: &str,
    port: u16,
    credentials: &Credentials,
    terminal: &mut dyn Terminal,
    limit: Duration,
) -> Result<i32, Error> {
    let (_handle, channel) = open(host, port, credentials, limit).await?;
    let (mut read, write) = channel.split();
    let kind = terminal.kind();
    let size = terminal.size();
    let (cols, rows) = size;
    let pty = write.request_pty(true, &kind, cols.into(), rows.into(), 0, 0, &[]);
    within(limit, pty, host).await?;
    granted(&mut read, terminal, limit, host).await?;
    within(limit, write.request_shell(true), host).await?;
    granted(&mut read, terminal, limit, host).await?;
    let keys = terminal
        .keys()
        .map_err(|error| Error::Library(error.to_string()))?;
    let write = Arc::new(write);
    send_keys(keys, Arc::clone(&write), tokio::runtime::Handle::current());
    pump(read, &write, terminal, size).await
}

async fn granted(
    read: &mut ChannelReadHalf,
    terminal: &mut dyn Terminal,
    limit: Duration,
    host: &str,
) -> Result<(), Error> {
    loop {
        match within(limit, async { Ok(read.wait().await) }, host).await? {
            Some(ChannelMsg::Success) => return Ok(()),
            Some(ChannelMsg::Failure) => return Err(Error::ShellRefused),
            Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                show(terminal, &data)?
            }
            Some(_) => {}
            None => return Err(Error::NoExitStatus),
        }
    }
}

fn send_keys(
    mut keys: Box<dyn Read + Send>,
    write: Arc<ChannelWriteHalf<client::Msg>>,
    runtime: tokio::runtime::Handle,
) {
    thread::spawn(move || {
        let mut buffer = [0; 4096];
        loop {
            match keys.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    if runtime
                        .block_on(write.data_bytes(buffer[..count].to_vec()))
                        .is_err()
                    {
                        return;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = runtime.block_on(write.eof());
    });
}

async fn pump(
    mut read: ChannelReadHalf,
    write: &ChannelWriteHalf<client::Msg>,
    terminal: &mut dyn Terminal,
    mut size: (u16, u16),
) -> Result<i32, Error> {
    let mut status = None;
    loop {
        if let Ok(message) = tokio::time::timeout(SIZE_CHECK, read.wait()).await {
            match message {
                Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                    show(terminal, &data)?
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => status = Some(exit_status as i32),
                Some(_) => {}
                None => return status.ok_or(Error::NoExitStatus),
            }
        }
        let now = terminal.size();
        if now != size {
            size = now;
            let _ = write.window_change(now.0.into(), now.1.into(), 0, 0).await;
        }
    }
}

fn show(terminal: &mut dyn Terminal, data: &[u8]) -> Result<(), Error> {
    let screen = terminal.screen();
    screen
        .write_all(data)
        .and_then(|()| screen.flush())
        .map_err(|error| Error::Library(error.to_string()))
}

async fn answer(
    channel: &mut Channel<client::Msg>,
    limit: Duration,
    host: &str,
) -> Result<Output, Error> {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut code = None;
    while let Some(message) = within(limit, async { Ok(channel.wait().await) }, host).await? {
        match message {
            ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, .. } => stderr.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status as i32),
            _ => {}
        }
    }
    Ok(Output {
        code: code.ok_or(Error::NoExitStatus)?,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

async fn within<T>(
    limit: Duration,
    step: impl Future<Output = Result<T, russh::Error>>,
    host: &str,
) -> Result<T, Error> {
    match tokio::time::timeout(limit, step).await {
        Ok(result) => result.map_err(|error| library(error, host)),
        Err(_) => Err(Error::NoAnswer),
    }
}

fn library(error: russh::Error, host: &str) -> Error {
    match error {
        russh::Error::IO(error) => Error::Unreachable {
            host: host.to_string(),
            reason: error.to_string(),
        },
        error => Error::Library(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::os::unix::net::UnixStream;
    use std::sync::{mpsc, Mutex};
    use std::thread;
    use std::time::Instant;

    use nobs_core::device::{FakeTerminal, Password};
    use russh::keys::{Algorithm, PrivateKey};
    use russh::server::{self, Auth, Msg, Server as _, Session};
    use russh::{ChannelId, Pty};

    use super::*;

    const LIMIT: Duration = Duration::from_millis(200);
    const EARLY: &[u8] = b"\x1b[1mnobs-t-early\x00\r\n";

    struct Device {
        _channel: Option<Channel<Msg>>,
        term: String,
        size: (u32, u32),
    }

    struct Devices;

    impl server::Server for Devices {
        type Handler = Device;

        fn new_client(&mut self, _: Option<SocketAddr>) -> Device {
            Device {
                _channel: None,
                term: String::new(),
                size: (0, 0),
            }
        }
    }

    impl server::Handler for Device {
        type Error = russh::Error;

        async fn auth_password(&mut self, _: &str, password: &str) -> Result<Auth, Self::Error> {
            if password == "hang" {
                return std::future::pending().await;
            }
            Ok(if password == "right" {
                Auth::Accept
            } else {
                Auth::reject()
            })
        }

        async fn channel_open_session(
            &mut self,
            channel: Channel<Msg>,
            reply: server::ChannelOpenHandle,
            _: &mut Session,
        ) -> Result<(), Self::Error> {
            self._channel = Some(channel);
            reply.accept().await;
            Ok(())
        }

        async fn exec_request(
            &mut self,
            channel: ChannelId,
            data: &[u8],
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            session.channel_success(channel)?;
            match data {
                b"answer" => {
                    session.data(channel, "out\n")?;
                    session.extended_data(channel, 1, "err\n")?;
                    session.exit_status_request(channel, 3)?;
                    session.close(channel)?;
                }
                b"no status" => {
                    session.data(channel, "out\n")?;
                    session.close(channel)?;
                }
                _ => {}
            }
            Ok(())
        }

        async fn pty_request(
            &mut self,
            channel: ChannelId,
            term: &str,
            col_width: u32,
            row_height: u32,
            _: u32,
            _: u32,
            _: &[(Pty, u32)],
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            match term {
                "nobs-t-refuse-pty" => session.channel_failure(channel)?,
                "nobs-t-silent-pty" => {}
                _ => session.channel_success(channel)?,
            }
            self.term = term.to_string();
            self.size = (col_width, row_height);
            Ok(())
        }

        async fn shell_request(
            &mut self,
            channel: ChannelId,
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            match self.term.as_str() {
                "nobs-t-refuse-shell" => return session.channel_failure(channel),
                "nobs-t-early-data" => session.data(channel, EARLY.to_vec())?,
                _ => {}
            }
            session.channel_success(channel)?;
            let (cols, rows) = self.size;
            session.data(channel, format!("{} {cols}x{rows}\n", self.term))
        }

        async fn data(
            &mut self,
            channel: ChannelId,
            data: &[u8],
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            match data {
                b"nobs-t-exit" => {
                    session.exit_status_request(channel, 7)?;
                    session.close(channel)
                }
                b"nobs-t-close" => session.close(channel),
                b"nobs-t-drop" => Err(russh::Error::Disconnect),
                _ => session.data(channel, data.to_vec()),
            }
        }

        async fn window_change_request(
            &mut self,
            channel: ChannelId,
            col_width: u32,
            row_height: u32,
            _: u32,
            _: u32,
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            session.data(channel, format!("{col_width}x{row_height}\n"))
        }
    }

    fn free_port() -> u16 {
        TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn device() -> u16 {
        let port = free_port();
        thread::spawn(move || {
            let config = Arc::new(server::Config {
                keys: vec![PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap()],
                auth_rejection_time: Duration::ZERO,
                ..Default::default()
            });
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(Devices.run_on_address(config, ("127.0.0.1", port)))
                .unwrap();
        });
        while TcpStream::connect(("127.0.0.1", port)).is_err() {
            thread::sleep(Duration::from_millis(10));
        }
        port
    }

    fn ask(port: u16, password: &str, command: &str) -> Result<Output, Error> {
        let credentials = Credentials {
            user: "admin".to_string(),
            password: Password::new(password.to_string()),
        };
        run_at("127.0.0.1", port, &credentials, command, LIMIT)
    }

    fn ask_in_five_seconds(
        port: u16,
        password: &'static str,
        command: &'static str,
    ) -> Result<Output, Error> {
        let (sender, answers) = mpsc::channel();
        thread::spawn(move || sender.send(ask(port, password, command)));
        answers
            .recv_timeout(Duration::from_secs(5))
            .expect("an answer within 5 seconds")
    }

    struct Shell {
        keys: UnixStream,
        screen: UnixStream,
        size: Arc<Mutex<(u16, u16)>>,
        ended: mpsc::Receiver<(Result<i32, Error>, bool)>,
    }

    impl Shell {
        fn types(&mut self, keys: &[u8]) {
            self.keys.write_all(keys).unwrap();
        }

        fn screen_shows(&mut self, expected: &[u8]) {
            let mut shown = vec![0; expected.len()];
            self.screen.read_exact(&mut shown).unwrap();
            assert_eq!(shown, expected);
        }

        fn end(&self) -> Result<i32, Error> {
            self.end_and_keys_taken().0
        }

        fn end_and_keys_taken(&self) -> (Result<i32, Error>, bool) {
            self.ended
                .recv_timeout(Duration::from_secs(5))
                .expect("an end within 5 seconds")
        }
    }

    fn shell(port: u16, password: &'static str, kind: &str) -> Shell {
        let (keys, device_keys) = UnixStream::pair().unwrap();
        let (screen, device_screen) = UnixStream::pair().unwrap();
        screen
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let size = Arc::new(Mutex::new((100, 30)));
        let mut terminal = FakeTerminal {
            kind: kind.to_string(),
            size: Arc::clone(&size),
            keys: Some(Box::new(device_keys)),
            screen: Box::new(device_screen),
        };
        let (sender, ended) = mpsc::channel();
        thread::spawn(move || {
            let credentials = Credentials {
                user: "admin".to_string(),
                password: Password::new(password.to_string()),
            };
            let end = shell_at("127.0.0.1", port, &credentials, &mut terminal, LIMIT);
            sender.send((end, terminal.keys.is_none()))
        });
        Shell {
            keys,
            screen,
            size,
            ended,
        }
    }

    #[test]
    fn a_closed_port_is_unreachable() {
        let port = free_port();
        let error = ask(port, "right", "answer").unwrap_err();
        assert!(
            matches!(&error, Error::Unreachable { host, .. } if host == "127.0.0.1"),
            "{error:?}"
        );
    }

    #[test]
    fn a_listener_that_never_speaks_gives_no_answer() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_eq!(
            ask_in_five_seconds(port, "right", "answer"),
            Err(Error::NoAnswer)
        );
    }

    #[test]
    fn an_answer_keeps_output_error_text_and_status_apart() {
        let port = device();
        assert_eq!(
            ask(port, "right", "answer"),
            Ok(Output {
                code: 3,
                stdout: "out\n".to_string(),
                stderr: "err\n".to_string(),
            })
        );
    }

    #[test]
    fn a_wrong_password_fails_authentication() {
        let port = device();
        assert_eq!(
            ask(port, "nobs-t-not-a-secret", "answer"),
            Err(Error::AuthenticationFailed)
        );
    }

    #[test]
    fn an_end_without_exit_status_is_an_error() {
        let port = device();
        assert_eq!(ask(port, "right", "no status"), Err(Error::NoExitStatus));
    }

    #[test]
    fn a_command_with_no_answer_gives_no_answer() {
        let port = device();
        assert_eq!(
            ask_in_five_seconds(port, "right", "silent"),
            Err(Error::NoAnswer)
        );
    }

    #[test]
    fn a_login_with_no_answer_gives_no_answer() {
        let port = device();
        assert_eq!(
            ask_in_five_seconds(port, "hang", "answer"),
            Err(Error::NoAnswer)
        );
    }

    #[test]
    fn every_byte_typed_reaches_the_device_and_every_byte_back_reaches_the_screen() {
        let mut shell = shell(device(), "right", "xterm");
        shell.screen_shows(b"xterm 100x30\n");
        let every_byte: Vec<u8> = (0..=255).collect();
        shell.types(&every_byte);
        shell.screen_shows(&every_byte);
        shell.types(b"nobs-t-exit");
        assert_eq!(shell.end_and_keys_taken(), (Ok(7), true));
    }

    #[test]
    fn bytes_the_device_sends_before_it_grants_the_shell_reach_the_screen_first() {
        let mut shell = shell(device(), "right", "nobs-t-early-data");
        shell.screen_shows(EARLY);
        shell.screen_shows(b"nobs-t-early-data 100x30\n");
        shell.types(b"nobs-t-exit");
        assert_eq!(shell.end(), Ok(7));
    }

    #[test]
    fn the_device_learns_the_first_size_the_terminal_type_and_every_new_size() {
        let mut shell = shell(device(), "right", "xterm-256color");
        shell.screen_shows(b"xterm-256color 100x30\n");
        *shell.size.lock().unwrap() = (80, 20);
        let changed = Instant::now();
        shell.screen_shows(b"80x20\n");
        assert!(changed.elapsed() < Duration::from_secs(1));
        shell.types(b"nobs-t-exit");
        assert_eq!(shell.end(), Ok(7));
    }

    #[test]
    fn the_shell_status_is_the_result_and_no_status_or_a_lost_connection_is_no_exit_status() {
        let cases = [
            ("nobs-t-exit", Ok(7)),
            ("nobs-t-close", Err(Error::NoExitStatus)),
            ("nobs-t-drop", Err(Error::NoExitStatus)),
        ];
        for (word, end) in cases {
            let mut shell = shell(device(), "right", "xterm");
            shell.screen_shows(b"xterm 100x30\n");
            shell.types(word.as_bytes());
            assert_eq!(shell.end(), end, "{word}");
        }
    }

    #[test]
    fn a_wrong_password_a_closed_port_and_a_silent_device_end_within_the_limit() {
        let silent = TcpListener::bind("127.0.0.1:0").unwrap();
        let cases = [
            (
                device(),
                "nobs-t-not-a-secret",
                "xterm",
                Error::AuthenticationFailed,
            ),
            (
                silent.local_addr().unwrap().port(),
                "right",
                "xterm",
                Error::NoAnswer,
            ),
            (device(), "hang", "xterm", Error::NoAnswer),
            (device(), "right", "nobs-t-silent-pty", Error::NoAnswer),
        ];
        for (case, (port, password, kind, error)) in cases.into_iter().enumerate() {
            assert_eq!(
                shell(port, password, kind).end_and_keys_taken(),
                (Err(error), false),
                "case {case}"
            );
        }
        let (closed, keys_taken) = shell(free_port(), "right", "xterm").end_and_keys_taken();
        assert!(
            matches!(&closed, Err(Error::Unreachable { host, .. }) if host == "127.0.0.1"),
            "{closed:?}"
        );
        assert!(!keys_taken);
    }

    #[test]
    fn an_open_shell_silent_beyond_the_limit_stays_open() {
        let mut shell = shell(device(), "right", "xterm");
        shell.screen_shows(b"xterm 100x30\n");
        thread::sleep(LIMIT * 4);
        assert_eq!(shell.ended.try_recv(), Err(mpsc::TryRecvError::Empty));
        shell.types(b"nobs-t-exit");
        assert_eq!(shell.end(), Ok(7));
    }

    #[test]
    fn a_refused_shell_is_shell_refused() {
        for kind in ["nobs-t-refuse-pty", "nobs-t-refuse-shell"] {
            assert_eq!(
                shell(device(), "right", kind).end_and_keys_taken(),
                (Err(Error::ShellRefused), false),
                "{kind}"
            );
        }
    }
}
