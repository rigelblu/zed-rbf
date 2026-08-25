use anyhow::Context as _;
use collections::HashMap;
use futures::{Future, FutureExt, channel::oneshot};
use parking_lot::{Mutex, RwLock};
use std::{
    marker::PhantomData,
    ops::Deref,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use thread_local::ThreadLocal;

use crate::{connection::Connection, domain::Migrator, util::UnboundedSyncSender};

const MIGRATION_RETRIES: usize = 10;
const CONNECTION_INITIALIZE_RETRIES: usize = 50;
const CONNECTION_INITIALIZE_RETRY_DELAY: Duration = Duration::from_millis(1);

pub enum QueuedWrite {
    Normal(Box<dyn 'static + Send + FnOnce()>),
    Transaction {
        activation: std::sync::mpsc::Receiver<()>,
        write: Box<dyn 'static + Send + FnOnce()>,
    },
}

impl QueuedWrite {
    fn run(self) {
        match self {
            Self::Normal(write) => write(),
            Self::Transaction { activation, write } => {
                if activation.recv().is_ok() {
                    write();
                }
            }
        }
    }
}
type WriteQueue = Box<dyn 'static + Send + Sync + Fn(QueuedWrite)>;
type WriteQueueConstructor = Box<dyn 'static + Send + FnMut() -> WriteQueue>;

enum TransactionCommand {
    Write(QueuedWriteTransaction),
    Commit(oneshot::Sender<anyhow::Result<()>>),
    CommitBlocking(std::sync::mpsc::SyncSender<anyhow::Result<()>>),
    Rollback(oneshot::Sender<anyhow::Result<()>>),
    RollbackBlocking(std::sync::mpsc::SyncSender<anyhow::Result<()>>),
}

enum TransactionReadySender {
    Async(oneshot::Sender<anyhow::Result<()>>),
    Blocking(std::sync::mpsc::SyncSender<anyhow::Result<()>>),
    Unobserved,
}

impl TransactionReadySender {
    fn send(self, result: anyhow::Result<()>) -> bool {
        match self {
            Self::Async(sender) => sender.send(result).is_ok(),
            Self::Blocking(sender) => sender.send(result).is_ok(),
            Self::Unobserved => true,
        }
    }
}

enum TransactionReadyReceiver {
    Async(oneshot::Receiver<anyhow::Result<()>>),
    Blocking(std::sync::mpsc::Receiver<anyhow::Result<()>>),
    Ready,
}

type QueuedWriteTransaction = Box<dyn 'static + Send + FnOnce(&Connection)>;

#[derive(Clone)]
pub struct WriteTransaction {
    commands: std::sync::mpsc::Sender<TransactionCommand>,
    activation: std::sync::mpsc::Sender<()>,
    activated: Arc<AtomicBool>,
    block_until_complete: bool,
}

impl WriteTransaction {
    pub fn activate(&self) -> anyhow::Result<()> {
        if !self.activated.swap(true, Ordering::SeqCst) {
            self.activation.send(()).map_err(|_| {
                anyhow::anyhow!("database write transaction could not be activated")
            })?;
        }
        Ok(())
    }

    pub async fn write<T: 'static + Send + Sync>(
        &self,
        callback: impl 'static + Send + FnOnce(&Connection) -> T,
    ) -> anyhow::Result<T> {
        self.activate()?;
        if self.block_until_complete {
            let (sender, receiver) = std::sync::mpsc::sync_channel(1);
            self.commands
                .send(TransactionCommand::Write(Box::new(move |connection| {
                    sender.send(callback(connection)).ok();
                })))
                .map_err(|_| anyhow::anyhow!("database write transaction is no longer active"))?;
            return receiver
                .recv()
                .map_err(|_| anyhow::anyhow!("database write transaction dropped a queued write"));
        }

        let (sender, receiver) = oneshot::channel();
        self.commands
            .send(TransactionCommand::Write(Box::new(move |connection| {
                sender.send(callback(connection)).ok();
            })))
            .map_err(|_| anyhow::anyhow!("database write transaction is no longer active"))?;
        receiver
            .await
            .map_err(|_| anyhow::anyhow!("database write transaction dropped a queued write"))
    }

    pub async fn commit(self) -> anyhow::Result<()> {
        self.activate()?;
        if self.block_until_complete {
            let (sender, receiver) = std::sync::mpsc::sync_channel(1);
            self.commands
                .send(TransactionCommand::CommitBlocking(sender))
                .map_err(|_| anyhow::anyhow!("database write transaction is no longer active"))?;
            return receiver
                .recv()
                .map_err(|_| anyhow::anyhow!("database write transaction dropped its commit"))?;
        }

        let (sender, receiver) = oneshot::channel();
        self.commands
            .send(TransactionCommand::Commit(sender))
            .map_err(|_| anyhow::anyhow!("database write transaction is no longer active"))?;
        receiver
            .await
            .map_err(|_| anyhow::anyhow!("database write transaction dropped its commit"))?
    }

    pub async fn rollback(self) -> anyhow::Result<()> {
        self.activate()?;
        if self.block_until_complete {
            let (sender, receiver) = std::sync::mpsc::sync_channel(1);
            self.commands
                .send(TransactionCommand::RollbackBlocking(sender))
                .map_err(|_| anyhow::anyhow!("database write transaction is no longer active"))?;
            return receiver
                .recv()
                .map_err(|_| anyhow::anyhow!("database write transaction dropped its rollback"))?;
        }

        let (sender, receiver) = oneshot::channel();
        self.commands
            .send(TransactionCommand::Rollback(sender))
            .map_err(|_| anyhow::anyhow!("database write transaction is no longer active"))?;
        receiver
            .await
            .map_err(|_| anyhow::anyhow!("database write transaction dropped its rollback"))?
    }
}

/// List of queues of tasks by database uri. This lets us serialize writes to the database
/// and have a single worker thread per db file. This means many thread safe connections
/// (possibly with different migrations) could all be communicating with the same background
/// thread.
static QUEUES: LazyLock<RwLock<HashMap<Arc<str>, WriteQueue>>> = LazyLock::new(Default::default);

/// Thread safe connection to a given database file or in memory db. This can be cloned, shared, static,
/// whatever. It derefs to a synchronous connection by thread that is read only. A write capable connection
/// may be accessed by passing a callback to the `write` function which will queue the callback
#[derive(Clone)]
pub struct ThreadSafeConnection {
    uri: Arc<str>,
    persistent: bool,
    connection_initialize_query: Option<&'static str>,
    connections: Arc<ThreadLocal<Connection>>,
    block_on_transaction_commands: bool,
}

unsafe impl Send for ThreadSafeConnection {}
unsafe impl Sync for ThreadSafeConnection {}

pub struct ThreadSafeConnectionBuilder<M: Migrator + 'static = ()> {
    db_initialize_query: Option<&'static str>,
    write_queue_constructor: Option<WriteQueueConstructor>,
    connection: ThreadSafeConnection,
    _migrator: PhantomData<*mut M>,
}

impl<M: Migrator> ThreadSafeConnectionBuilder<M> {
    /// Sets the query to run every time a connection is opened. This must
    /// be infallible (EG only use pragma statements) and not cause writes.
    /// to the db or it will panic.
    pub fn with_connection_initialize_query(mut self, initialize_query: &'static str) -> Self {
        self.connection.connection_initialize_query = Some(initialize_query);
        self
    }

    /// Queues an initialization query for the database file. This must be infallible
    /// but may cause changes to the database file such as with `PRAGMA journal_mode`
    pub fn with_db_initialization_query(mut self, initialize_query: &'static str) -> Self {
        self.db_initialize_query = Some(initialize_query);
        self
    }

    /// Specifies how the thread safe connection should serialize writes. If provided
    /// the connection will call the write_queue_constructor for each database file in
    /// this process. The constructor is responsible for setting up a background thread or
    /// async task which handles queued writes with the provided connection.
    pub fn with_write_queue_constructor(
        mut self,
        write_queue_constructor: WriteQueueConstructor,
    ) -> Self {
        self.write_queue_constructor = Some(write_queue_constructor);
        self
    }

    pub fn with_locking_write_queue(mut self) -> Self {
        self.write_queue_constructor = Some(locking_queue());
        self.connection.block_on_transaction_commands = true;
        self
    }

    pub async fn build(self) -> anyhow::Result<ThreadSafeConnection> {
        self.connection
            .initialize_queues(self.write_queue_constructor);

        let db_initialize_query = self.db_initialize_query;

        self.connection
            .write(move |connection| {
                if let Some(db_initialize_query) = db_initialize_query {
                    connection.exec(db_initialize_query).with_context(|| {
                        format!(
                            "Db initialize query failed to execute: {}",
                            db_initialize_query
                        )
                    })?()?;
                }

                // Retry failed migrations in case they were run in parallel from different
                // processes. This gives a best attempt at migrating before bailing
                let mut migration_result =
                    anyhow::Result::<()>::Err(anyhow::anyhow!("Migration never run"));

                let foreign_keys_enabled: bool =
                    connection.select_row::<i32>("PRAGMA foreign_keys")?()
                        .unwrap_or(None)
                        .map(|enabled| enabled != 0)
                        .unwrap_or(false);

                connection.exec("PRAGMA foreign_keys = OFF;")?()?;

                for _ in 0..MIGRATION_RETRIES {
                    migration_result = connection
                        .with_savepoint("thread_safe_multi_migration", || M::migrate(connection));

                    if migration_result.is_ok() {
                        break;
                    }
                }

                if foreign_keys_enabled {
                    connection.exec("PRAGMA foreign_keys = ON;")?()?;
                }
                migration_result
            })
            .await?;

        Ok(self.connection)
    }
}

impl ThreadSafeConnection {
    fn initialize_queues(&self, write_queue_constructor: Option<WriteQueueConstructor>) -> bool {
        if !QUEUES.read().contains_key(&self.uri) {
            let mut queues = QUEUES.write();
            if !queues.contains_key(&self.uri) {
                let mut write_queue_constructor =
                    write_queue_constructor.unwrap_or_else(background_thread_queue);
                queues.insert(self.uri.clone(), write_queue_constructor());
                return true;
            }
        }
        false
    }

    pub fn builder<M: Migrator>(uri: &str, persistent: bool) -> ThreadSafeConnectionBuilder<M> {
        ThreadSafeConnectionBuilder::<M> {
            db_initialize_query: None,
            write_queue_constructor: None,
            connection: Self {
                uri: Arc::from(uri),
                persistent,
                connection_initialize_query: None,
                connections: Default::default(),
                block_on_transaction_commands: false,
            },
            _migrator: PhantomData,
        }
    }

    /// Opens a new db connection with the initialized file path. This is internal and only
    /// called from the deref function.
    fn open_file(uri: &str) -> Connection {
        Connection::open_file(uri)
    }

    /// Opens a shared memory connection using the file path as the identifier. This is internal
    /// and only called from the deref function.
    fn open_shared_memory(uri: &str) -> Connection {
        Connection::open_memory(Some(uri))
    }

    pub fn write<T: 'static + Send + Sync>(
        &self,
        callback: impl 'static + Send + FnOnce(&Connection) -> T,
    ) -> impl Future<Output = T> {
        // Check and invalidate queue and maybe recreate queue
        let queues = QUEUES.read();
        let write_channel = queues
            .get(&self.uri)
            .expect("Queues are inserted when build is called. This should always succeed");

        // Create a one shot channel for the result of the queued write
        // so we can await on the result
        let (sender, receiver) = oneshot::channel();

        let thread_safe_connection = (*self).clone();
        write_channel(QueuedWrite::Normal(Box::new(move || {
            let connection = thread_safe_connection.deref();
            let result = connection.with_write(|connection| callback(connection));
            sender.send(result).ok();
        })));
        receiver.map(|response| response.expect("Write queue unexpectedly closed"))
    }

    pub fn queue_write_transaction(
        &self,
    ) -> (
        WriteTransaction,
        futures::future::BoxFuture<'static, anyhow::Result<()>>,
    ) {
        self.queue_write_transaction_inner(false)
    }

    fn queue_write_transaction_inner(
        &self,
        block_until_started: bool,
    ) -> (
        WriteTransaction,
        futures::future::BoxFuture<'static, anyhow::Result<()>>,
    ) {
        const SAVEPOINT: &str = "thread_safe_connection_write_transaction";

        let (commands, command_receiver) = std::sync::mpsc::channel();
        let (activation, activation_receiver) = std::sync::mpsc::channel();
        let block_until_complete = self.block_on_transaction_commands;
        let (ready_sender, ready_receiver) = if block_until_started {
            let (sender, receiver) = std::sync::mpsc::sync_channel(1);
            (
                TransactionReadySender::Blocking(sender),
                TransactionReadyReceiver::Blocking(receiver),
            )
        } else if block_until_complete {
            (
                TransactionReadySender::Unobserved,
                TransactionReadyReceiver::Ready,
            )
        } else {
            let (sender, receiver) = oneshot::channel();
            (
                TransactionReadySender::Async(sender),
                TransactionReadyReceiver::Async(receiver),
            )
        };
        let transaction = WriteTransaction {
            commands,
            activation,
            activated: Arc::new(AtomicBool::new(false)),
            block_until_complete,
        };
        let queues = QUEUES.read();
        let write_channel = queues
            .get(&self.uri)
            .expect("Queues are inserted when build is called. This should always succeed");
        let thread_safe_connection = self.clone();
        write_channel(QueuedWrite::Transaction {
            activation: activation_receiver,
            write: Box::new(move || {
                let connection = thread_safe_connection.deref();
                connection.with_write(|connection| {
                    let begin_result = connection
                        .exec(&format!("SAVEPOINT {SAVEPOINT}"))
                        .and_then(|mut statement| statement());
                    if let Err(error) = begin_result {
                        ready_sender.send(Err(error));
                        return;
                    }
                    if !ready_sender.send(Ok(())) {
                        rollback_transaction(connection, SAVEPOINT).ok();
                        return;
                    }

                    while let Ok(command) = command_receiver.recv() {
                        match command {
                            TransactionCommand::Write(write) => write(connection),
                            TransactionCommand::Commit(completion) => {
                                let result = connection
                                    .exec(&format!("RELEASE SAVEPOINT {SAVEPOINT}"))
                                    .and_then(|mut statement| statement());
                                completion.send(result).ok();
                                return;
                            }
                            TransactionCommand::CommitBlocking(completion) => {
                                let result = connection
                                    .exec(&format!("RELEASE SAVEPOINT {SAVEPOINT}"))
                                    .and_then(|mut statement| statement());
                                completion.send(result).ok();
                                return;
                            }
                            TransactionCommand::Rollback(completion) => {
                                completion
                                    .send(rollback_transaction(connection, SAVEPOINT))
                                    .ok();
                                return;
                            }
                            TransactionCommand::RollbackBlocking(completion) => {
                                completion
                                    .send(rollback_transaction(connection, SAVEPOINT))
                                    .ok();
                                return;
                            }
                        }
                    }

                    rollback_transaction(connection, SAVEPOINT).ok();
                });
            }),
        });
        if block_until_started && let Err(error) = transaction.activate() {
            return (transaction, futures::future::ready(Err(error)).boxed());
        }
        let ready = match ready_receiver {
            TransactionReadyReceiver::Async(receiver) => async move {
                receiver
                    .await
                    .map_err(|_| anyhow::anyhow!("database write transaction failed to start"))?
            }
            .boxed(),
            TransactionReadyReceiver::Blocking(receiver) => {
                let result = receiver.recv().unwrap_or_else(|_| {
                    Err(anyhow::anyhow!(
                        "database write transaction failed to start"
                    ))
                });
                futures::future::ready(result).boxed()
            }
            TransactionReadyReceiver::Ready => futures::future::ready(Ok(())).boxed(),
        };
        (transaction, ready)
    }

    pub async fn begin_write_transaction(&self) -> anyhow::Result<WriteTransaction> {
        let (transaction, ready) =
            self.queue_write_transaction_inner(self.block_on_transaction_commands);
        transaction.activate()?;
        ready.await?;
        Ok(transaction)
    }

    pub(crate) fn create_connection(
        persistent: bool,
        uri: &str,
        connection_initialize_query: Option<&'static str>,
    ) -> Connection {
        let mut connection = if persistent {
            Self::open_file(uri)
        } else {
            Self::open_shared_memory(uri)
        };

        if let Some(initialize_query) = connection_initialize_query {
            let mut last_error = None;
            let initialized = (0..CONNECTION_INITIALIZE_RETRIES).any(|attempt| {
                match connection
                    .exec(initialize_query)
                    .and_then(|mut statement| statement())
                {
                    Ok(()) => true,
                    Err(err)
                        if is_schema_lock_error(&err)
                            && attempt + 1 < CONNECTION_INITIALIZE_RETRIES =>
                    {
                        last_error = Some(err);
                        thread::sleep(CONNECTION_INITIALIZE_RETRY_DELAY);
                        false
                    }
                    Err(err) => {
                        panic!(
                            "Initialize query failed to execute: {}\n\nCaused by:\n{err:#}",
                            initialize_query
                        )
                    }
                }
            });

            if !initialized {
                let err = last_error
                    .expect("connection initialization retries should record the last error");
                panic!(
                    "Initialize query failed to execute after retries: {}\n\nCaused by:\n{err:#}",
                    initialize_query
                );
            }
        }

        // Disallow writes on the connection. The only writes allowed for thread safe connections
        // are from the background thread that can serialize them.
        *connection.write.get_mut() = false;

        connection
    }
}

fn rollback_transaction(connection: &Connection, savepoint: &str) -> anyhow::Result<()> {
    connection.exec(&format!("ROLLBACK TO SAVEPOINT {savepoint}"))?()?;
    connection.exec(&format!("RELEASE SAVEPOINT {savepoint}"))?()?;
    Ok(())
}

fn is_schema_lock_error(err: &anyhow::Error) -> bool {
    let message = format!("{err:#}");
    message.contains("database schema is locked") || message.contains("database is locked")
}

impl ThreadSafeConnection {
    /// Special constructor for ThreadSafeConnection which disallows db initialization and migrations.
    /// This allows construction to be infallible and not write to the db.
    pub fn new(
        uri: &str,
        persistent: bool,
        connection_initialize_query: Option<&'static str>,
        write_queue_constructor: Option<WriteQueueConstructor>,
    ) -> Self {
        let connection = Self {
            uri: Arc::from(uri),
            persistent,
            connection_initialize_query,
            connections: Default::default(),
            block_on_transaction_commands: false,
        };

        connection.initialize_queues(write_queue_constructor);
        connection
    }
}

impl Deref for ThreadSafeConnection {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        self.connections.get_or(|| {
            Self::create_connection(self.persistent, &self.uri, self.connection_initialize_query)
        })
    }
}

pub fn background_thread_queue() -> WriteQueueConstructor {
    use std::sync::mpsc::channel;

    Box::new(|| {
        let (sender, receiver) = channel::<QueuedWrite>();

        thread::Builder::new()
            .name("sqlezWorker".to_string())
            .spawn(move || {
                while let Ok(write) = receiver.recv() {
                    write.run()
                }
            })
            .unwrap();

        let sender = UnboundedSyncSender::new(sender);
        Box::new(move |queued_write| {
            sender
                .send(queued_write)
                .expect("Could not send write action to background thread");
        })
    })
}

pub fn locking_queue() -> WriteQueueConstructor {
    Box::new(|| {
        let write_mutex = Arc::new(Mutex::new(()));
        Box::new(move |queued_write| {
            let write_mutex = write_mutex.clone();
            match queued_write {
                QueuedWrite::Normal(write) => {
                    let _lock = write_mutex.lock();
                    write();
                }
                QueuedWrite::Transaction { activation, write } => {
                    thread::spawn(move || {
                        if activation.recv().is_ok() {
                            let _lock = write_mutex.lock();
                            write();
                        }
                    });
                }
            }
        })
    })
}

#[cfg(test)]
mod test {
    use indoc::indoc;
    use std::ops::Deref;

    use std::{thread, time::Duration};

    use crate::{domain::Domain, thread_safe_connection::ThreadSafeConnection};

    #[test]
    fn many_initialize_and_migrate_queries_at_once() {
        let mut handles = vec![];

        enum TestDomain {}
        impl Domain for TestDomain {
            const NAME: &str = "test";
            const MIGRATIONS: &[&str] = &["CREATE TABLE test(col1 TEXT, col2 TEXT) STRICT;"];
        }

        for _ in 0..100 {
            handles.push(thread::spawn(|| {
                let builder =
                    ThreadSafeConnection::builder::<TestDomain>("annoying-test.db", false)
                        .with_db_initialization_query("PRAGMA journal_mode=WAL")
                        .with_connection_initialize_query(indoc! {"
                                PRAGMA synchronous=NORMAL;
                                PRAGMA busy_timeout=1;
                                PRAGMA foreign_keys=TRUE;
                                PRAGMA case_sensitive_like=TRUE;
                            "});

                let _ = pollster::block_on(builder.build()).unwrap().deref();
            }));
        }

        for handle in handles {
            let _ = handle.join();
        }
    }

    #[test]
    fn connection_initialize_query_retries_transient_schema_lock() {
        let name = "connection_initialize_query_retries_transient_schema_lock";
        let locking_connection = crate::connection::Connection::open_memory(Some(name));
        locking_connection.exec("BEGIN IMMEDIATE").unwrap()().unwrap();
        locking_connection
            .exec("CREATE TABLE test(col TEXT)")
            .unwrap()()
        .unwrap();

        let releaser = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            locking_connection.exec("ROLLBACK").unwrap()().unwrap();
        });

        ThreadSafeConnection::create_connection(false, name, Some("PRAGMA FOREIGN_KEYS=true"));
        releaser.join().unwrap();
    }

    #[test]
    fn write_transaction_commits_or_rolls_back_as_one_unit() {
        let connection = pollster::block_on(
            ThreadSafeConnection::builder::<()>(
                "write_transaction_commits_or_rolls_back_as_one_unit",
                false,
            )
            .with_locking_write_queue()
            .build(),
        )
        .unwrap();
        pollster::block_on(connection.write(|connection| {
            connection.exec("CREATE TABLE values_table(value INTEGER)")?()?;
            anyhow::Ok(())
        }))
        .unwrap();

        let transaction = pollster::block_on(connection.begin_write_transaction()).unwrap();
        pollster::block_on(transaction.write(|connection| {
            connection.exec("INSERT INTO values_table VALUES (1)")?()?;
            anyhow::Ok(())
        }))
        .unwrap()
        .unwrap();
        pollster::block_on(transaction.rollback()).unwrap();
        assert_eq!(
            connection
                .select_row::<i64>("SELECT COUNT(*) FROM values_table")
                .unwrap()()
            .unwrap(),
            Some(0)
        );

        let transaction = pollster::block_on(connection.begin_write_transaction()).unwrap();
        pollster::block_on(transaction.write(|connection| {
            connection.exec("INSERT INTO values_table VALUES (2)")?()?;
            anyhow::Ok(())
        }))
        .unwrap()
        .unwrap();
        pollster::block_on(transaction.commit()).unwrap();
        assert_eq!(
            connection
                .select_row::<i64>("SELECT value FROM values_table")
                .unwrap()()
            .unwrap(),
            Some(2)
        );
    }
}
