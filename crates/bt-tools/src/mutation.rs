use camino::{Utf8Path, Utf8PathBuf};
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex, OnceLock},
};
use tokio::sync::Mutex as AsyncMutex;

type QueueMap = Mutex<HashMap<Utf8PathBuf, Arc<AsyncMutex<()>>>>;

fn mutation_queues() -> &'static QueueMap {
    static QUEUES: OnceLock<QueueMap> = OnceLock::new();
    QUEUES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn mutation_queue_key(path: &Utf8Path) -> Utf8PathBuf {
    path.canonicalize_utf8().unwrap_or_else(|_| path.to_owned())
}

pub async fn with_file_mutation_queue<T, F>(path: &Utf8Path, operation: F) -> T
where
    F: Future<Output = T>,
{
    let key = mutation_queue_key(path);
    let queue = {
        let mut queues = mutation_queues()
            .lock()
            .expect("file mutation queue lock poisoned");
        queues
            .entry(key.clone())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    };

    let result = {
        let _guard = queue.lock().await;
        operation.await
    };

    let mut queues = mutation_queues()
        .lock()
        .expect("file mutation queue lock poisoned");
    if Arc::strong_count(&queue) == 2
        && queues
            .get(&key)
            .is_some_and(|candidate| Arc::ptr_eq(candidate, &queue))
    {
        queues.remove(&key);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::with_file_mutation_queue;
    use camino::Utf8PathBuf;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::Mutex as AsyncMutex;
    use tokio::time::{Duration, sleep};

    #[tokio::test]
    async fn same_file_operations_are_serialized() {
        let root = TempDir::new().expect("tempdir");
        let path = Utf8PathBuf::from_path_buf(root.path().join("same.txt")).expect("utf8");
        let order = Arc::new(AsyncMutex::new(Vec::<String>::new()));

        let first_order = order.clone();
        let first_path = path.clone();
        let first = tokio::spawn(async move {
            with_file_mutation_queue(&first_path, async move {
                first_order.lock().await.push("first:start".to_owned());
                sleep(Duration::from_millis(30)).await;
                first_order.lock().await.push("first:end".to_owned());
            })
            .await;
        });

        let second_order = order.clone();
        let second_path = path.clone();
        let second = tokio::spawn(async move {
            with_file_mutation_queue(&second_path, async move {
                second_order.lock().await.push("second:start".to_owned());
                second_order.lock().await.push("second:end".to_owned());
            })
            .await;
        });

        first.await.expect("first join");
        second.await.expect("second join");

        assert_eq!(
            *order.lock().await,
            vec![
                "first:start".to_owned(),
                "first:end".to_owned(),
                "second:start".to_owned(),
                "second:end".to_owned()
            ]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_aliases_share_a_queue() {
        let root = TempDir::new().expect("tempdir");
        let target = root.path().join("target.txt");
        std::fs::write(&target, "hello\n").expect("write target");
        let alias = root.path().join("alias.txt");
        std::os::unix::fs::symlink(&target, &alias).expect("create symlink");

        let target = Utf8PathBuf::from_path_buf(target).expect("utf8");
        let alias = Utf8PathBuf::from_path_buf(alias).expect("utf8");
        let order = Arc::new(AsyncMutex::new(Vec::<String>::new()));

        let first_order = order.clone();
        let first_target = target.clone();
        let first = tokio::spawn(async move {
            with_file_mutation_queue(&first_target, async move {
                first_order.lock().await.push("target:start".to_owned());
                sleep(Duration::from_millis(30)).await;
                first_order.lock().await.push("target:end".to_owned());
            })
            .await;
        });

        let second_order = order.clone();
        let second_alias = alias.clone();
        let second = tokio::spawn(async move {
            with_file_mutation_queue(&second_alias, async move {
                second_order.lock().await.push("alias:start".to_owned());
                second_order.lock().await.push("alias:end".to_owned());
            })
            .await;
        });

        first.await.expect("first join");
        second.await.expect("second join");

        assert_eq!(
            *order.lock().await,
            vec![
                "target:start".to_owned(),
                "target:end".to_owned(),
                "alias:start".to_owned(),
                "alias:end".to_owned()
            ]
        );
    }
}
