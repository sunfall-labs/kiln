//! `--one-shot`: run the pipeline in this process and leave nothing behind.
//!
//! CI and reproducible builds must not depend on a background process that might be warm,
//! cold, or holding someone else's edits, so every command can be run against a database that
//! is created and destroyed by the invocation itself.
//!
//! The engine runs in this process, but it is still reached over a private socket in a private
//! temporary directory rather than by calling the dispatcher directly, because `kiln-daemon`
//! does not export one: `dispatch` is crate-private and `MemoEngine` is not re-exported, so a
//! direct in-process call is not available to a client that only depends on `kiln-daemon`. The
//! loopback costs a few microseconds per request against a check that takes milliseconds, and
//! it has the compensating virtue that one-shot and daemon mode exercise exactly the same
//! code path. If `kiln-daemon` later exports its dispatcher, this type shrinks to a wrapper
//! around it and nothing else in the CLI changes.

use std::path::PathBuf;
use std::thread::JoinHandle;

use kiln_daemon::{Daemon, ShutdownHandle};
use kiln_proto::{Request, Response};

use crate::backend::Backend;
use crate::client::DaemonClient;
use crate::error::{Error, Result};

pub struct OneShot {
    client: DaemonClient,
    socket: PathBuf,
    stop: ShutdownHandle,
    serving: Option<JoinHandle<()>>,
    dir: PathBuf,
}

impl OneShot {
    pub fn start() -> Result<OneShot> {
        // Named by pid so two concurrent `kiln --one-shot` runs cannot share a database, and
        // cleared first in case a killed run of the same pid left the directory behind.
        let dir = std::env::temp_dir().join(format!("kiln-1s-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("daemon.sock");

        let daemon = Daemon::bind(&socket).map_err(|source| {
            Error::Daemon(format!("cannot start an in-process engine: {source}"))
        })?;
        let stop = daemon.shutdown_handle();
        let serving = std::thread::spawn(move || daemon.serve());

        // `bind` already returned, so the socket is accepting: no polling needed here.
        match DaemonClient::connect(&socket).and_then(|mut client| {
            client.handshake()?;
            Ok(client)
        }) {
            Ok(client) => Ok(OneShot {
                client,
                socket,
                stop,
                serving: Some(serving),
                dir,
            }),
            Err(error) => {
                stop.shutdown();
                let _ = serving.join();
                let _ = std::fs::remove_dir_all(&dir);
                Err(error)
            }
        }
    }
}

impl Backend for OneShot {
    fn peer(&self) -> Result<Box<dyn Backend>> {
        let mut peer = DaemonClient::connect(&self.socket)?;
        peer.handshake()?;
        Ok(Box::new(peer))
    }

    fn request(&mut self, request: Request) -> Result<Response> {
        self.client.request(request)
    }
}

impl Drop for OneShot {
    fn drop(&mut self) {
        self.stop.shutdown();
        if let Some(serving) = self.serving.take() {
            let _ = serving.join();
        }
        // The engine was private to this invocation; leaving its socket directory behind
        // would turn "reproducible" into "accumulates litter in $TMPDIR".
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands;

    #[test]
    fn a_one_shot_run_answers_and_then_leaves_nothing_behind() {
        let fixture =
            std::env::temp_dir().join(format!("kiln-cli-1s-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&fixture);
        std::fs::create_dir_all(&fixture).expect("create fixture");
        let source = fixture.join("a.js");
        std::fs::write(&source, "export const a = 1;\n").expect("write source");

        let dir = {
            let mut session = OneShot::start().expect("start an in-process engine");
            let out = commands::check(&mut session, &source, &fixture)
                .expect("check")
                .text;
            assert!(out.contains("a.js  r"), "{out}");
            // A fresh database has computed nothing before, so this check is all new work.
            assert!(out.contains("recomputed      parse 1"), "{out}");
            session.dir.clone()
        };

        assert!(
            !dir.exists(),
            "{} should have been cleaned up",
            dir.display()
        );
        let _ = std::fs::remove_dir_all(&fixture);
    }
}
