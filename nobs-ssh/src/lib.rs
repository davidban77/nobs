use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use nobs_core::device::{Credentials, Error, Transport, ANSWER_LIMIT};
use nobs_core::runner::Output;
use russh::keys::PublicKeyOrCertificate;
use russh::{client, Channel, ChannelMsg};

pub struct Ssh;

impl Transport for Ssh {
    fn run(&self, host: &str, credentials: &Credentials, command: &str) -> Result<Output, Error> {
        run_at(host, 22, credentials, command, ANSWER_LIMIT)
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
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| Error::Library(error.to_string()))?
        .block_on(session(host, port, credentials, command, limit))
}

async fn session(
    host: &str,
    port: u16,
    credentials: &Credentials,
    command: &str,
    limit: Duration,
) -> Result<Output, Error> {
    let config = Arc::new(client::Config::default());
    let connection = client::connect(config, (host, port), AcceptAnyKey);
    let mut handle = within(limit, connection, host).await?;
    let password = credentials.password.expose();
    let login = handle.authenticate_password(credentials.user.as_str(), password);
    if !within(limit, login, host).await?.success() {
        return Err(Error::AuthenticationFailed);
    }
    let mut channel = within(limit, handle.channel_open_session(), host).await?;
    within(limit, channel.exec(true, command), host).await?;
    answer(&mut channel, limit, host).await
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
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::thread;

    use nobs_core::device::Password;
    use russh::keys::{Algorithm, PrivateKey};
    use russh::server::{self, Auth, Msg, Server as _, Session};
    use russh::ChannelId;

    use super::*;

    const LIMIT: Duration = Duration::from_millis(200);

    struct Device {
        _channel: Option<Channel<Msg>>,
    }

    struct Devices;

    impl server::Server for Devices {
        type Handler = Device;

        fn new_client(&mut self, _: Option<SocketAddr>) -> Device {
            Device { _channel: None }
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
}
