//! Persistence-boundary regressions. Injected I/O failures establish ordering
//! and error propagation, not physical power-cut durability.
use super::*;

const OLD: &[u8] = br#"{"record":"old","complete":true}"#;
const NEW: &[u8] = br#"{"record":"new","complete":true}"#;

fn stage(io: &ReplaceIo<'_>) -> &'static str {
    match io {
        ReplaceIo::Write(..) => "write",
        ReplaceIo::FileSync(..) => "file-sync",
        ReplaceIo::Rename(..) => "rename",
        ReplaceIo::DirectorySync(..) => "directory-sync",
    }
}

#[test]
fn success_acknowledges_only_write_file_sync_rename_directory_sync() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("record.json");
    std::fs::write(&path, OLD).unwrap();
    let mut completed = Vec::new();
    let result = write_atomic_with(&path, NEW, None, |io| {
        let name = stage(&io);
        if name == "directory-sync" {
            // Rename has already published a complete record, not an empty file.
            assert_eq!(std::fs::read(&path).unwrap(), NEW);
        }
        io.run()?;
        completed.push(name);
        Ok(())
    });
    assert!(result.is_ok());
    assert_eq!(
        completed,
        ["write", "file-sync", "rename", "directory-sync"]
    );
    assert_eq!(std::fs::read(&path).unwrap(), NEW);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn each_persistence_failure_is_explicit_and_reopen_never_reads_partial_bytes() {
    let sequence = ["write", "file-sync", "rename", "directory-sync"];
    for existed in [false, true] {
        for (index, failure) in sequence.iter().enumerate() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("record.json");
            if existed {
                std::fs::write(&path, OLD).unwrap();
            }
            let mut attempted = Vec::new();
            let result = write_atomic_with(&path, NEW, None, |io| {
                let name = stage(&io);
                attempted.push(name);
                if name == *failure {
                    if let ReplaceIo::Write(file, bytes) = io {
                        // A write can fail after transferring only a prefix.
                        file.write_all(&bytes[..5])?;
                    }
                    return Err(std::io::Error::other(format!("injected {name}")));
                }
                io.run()
            });
            let error = format!("{:#}", result.unwrap_err());
            assert!(error.contains(&path.display().to_string()), "{error}");
            assert!(error.contains(&format!("injected {failure}")), "{error}");
            assert_eq!(attempted, sequence[..=index]);
            if index == 3 || existed {
                let expected = if index == 3 { NEW } else { OLD };
                assert_eq!(std::fs::read(&path).unwrap(), expected);
                let reopened: serde_json::Value =
                    serde_json::from_reader(File::open(&path).unwrap()).unwrap();
                assert_eq!(reopened["complete"], true);
            } else {
                assert_eq!(
                    File::open(&path).unwrap_err().kind(),
                    std::io::ErrorKind::NotFound
                );
            }
            // Failure cleanup removes only our temp, even after a partial write.
            assert_eq!(
                std::fs::read_dir(dir.path()).unwrap().count(),
                usize::from(path.exists())
            );
        }
    }
}

#[test]
fn competing_locked_writers_use_distinct_temp_names_and_complete_records() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("record.json");
    let lock = dir.path().join("writer.lock");
    let barrier = std::sync::Barrier::new(8);
    let names = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for writer in 0..8 {
            let (path, lock, barrier, names) = (&path, &lock, &barrier, &names);
            scope.spawn(move || {
                barrier.wait();
                let _lock = lock_file(lock).unwrap();
                let bytes = serde_json::to_vec(&serde_json::json!({"writer": writer})).unwrap();
                write_atomic_with(path, &bytes, None, |io| {
                    if let ReplaceIo::Rename(tmp, destination) = &io {
                        assert_eq!(*destination, path);
                        assert_eq!(tmp.parent(), path.parent());
                        names.lock().unwrap().push(tmp.to_path_buf());
                    }
                    io.run()
                })
                .unwrap();
                assert_eq!(std::fs::read(path).unwrap(), bytes);
            });
        }
    });
    let names = names.into_inner().unwrap();
    assert_eq!(names.len(), 8);
    assert_eq!(
        names
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        8
    );
    let record: serde_json::Value = serde_json::from_reader(File::open(&path).unwrap()).unwrap();
    assert!(record["writer"].as_u64().unwrap() < 8);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
}

#[test]
fn simultaneous_temp_reservations_are_exclusive_and_do_not_truncate_leftovers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("record.json");
    // The old pid-only name could survive a crash and get reused by replacement.
    let leftover = dir
        .path()
        .join(format!(".record.json.{}.tmp", std::process::id()));
    std::fs::write(&leftover, OLD).unwrap();
    let barrier = std::sync::Barrier::new(8);
    let names = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let (path, barrier) = (&path, &barrier);
                scope.spawn(move || {
                    let (tmp, mut file) = unique_temp(path).unwrap();
                    file.write_all(NEW).unwrap();
                    barrier.wait();
                    assert_eq!(std::fs::read(&tmp).unwrap(), NEW);
                    tmp
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        names
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        8
    );
    write_atomic(&path, NEW).unwrap();
    assert_eq!(std::fs::read(&leftover).unwrap(), OLD);
}

#[test]
fn exclusive_temp_creation_skips_a_crash_leftover_without_truncating_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("record.json");
    let leftover = dir
        .path()
        .join(format!(".record.json.{}.0.tmp", std::process::id()));
    std::fs::write(&leftover, OLD).unwrap();
    let serial = std::sync::atomic::AtomicU64::new(0);
    let (tmp, mut file) = unique_temp_with(&path, &serial).unwrap();
    file.write_all(NEW).unwrap();
    assert_ne!(tmp, leftover);
    assert_eq!(std::fs::read(&leftover).unwrap(), OLD);
    assert_eq!(std::fs::read(tmp).unwrap(), NEW);
}

#[test]
fn historical_state_plan_roundtrip_keeps_records_and_done_counts() {
    let root = tempfile::tempdir().unwrap();
    let project = create(root.path(), "demo", "", vec![]).unwrap();
    let path = crate::plan::plan_path(&project);
    // Schema 1, before subtasks/prerequisite edges. No reader or schema changes.
    let historical = b"schema = 1\nrevision = 3\nnext_step = 3\ngoal = \"\"\nkind = \"screen\"\nwhat_you_get = \"A screen you open.\"\ndoes = \"It shows the result.\"\n\n[[steps]]\nid = \"s-1\"\ntext = \"Build the screen\"\nstate = \"done\"\ntasks = []\nthreads = []\n\n[[steps]]\nid = \"s-2\"\ntext = \"Try it out\"\nstate = \"left\"\ntasks = []\nthreads = []\n";
    std::fs::write(&path, historical).unwrap();
    let before = crate::plan::load(&project).unwrap().unwrap();
    let count = |plan: &crate::contracts::Plan| {
        crate::plan::all_steps(plan)
            .filter(|step| step.state == crate::contracts::StepState::Done)
            .count()
    };
    assert_eq!(count(&before), 1);
    {
        let _lock = lock_file(&project.state_dir().join("plan.lock")).unwrap();
        write_atomic(&path, toml::to_string(&before).unwrap().as_bytes()).unwrap();
    }
    let reopened = Project::load(root.path(), "demo").unwrap();
    let after = crate::plan::load(&reopened).unwrap().unwrap();
    assert_eq!(after, before);
    assert_eq!(count(&after), 1);
}

#[test]
fn d30_record_damage_during_replacement_sync_is_not_overwritten() {
    for damage in [Some(b"{invalid wall record\n".as_slice()), None] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t-0001.toml");
        std::fs::write(&path, OLD).unwrap();
        let result = write_atomic_with(&path, NEW, Some(OLD), |io| {
            let sync = matches!(io, ReplaceIo::FileSync(_));
            io.run()?;
            if sync {
                // The replacement is complete and synced, but not published.
                if let Some(bytes) = damage {
                    std::fs::write(&path, bytes)?;
                } else {
                    std::fs::remove_file(&path)?;
                }
            }
            Ok(())
        });
        let error = format!("{:#}", result.unwrap_err());
        assert!(error.contains(&path.display().to_string()), "{error}");
        if let Some(bytes) = damage {
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        } else {
            assert!(!path.exists(), "deleted record was resurrected");
        }
        // Refusal also removes the staged replacement, not the damaged record.
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            usize::from(damage.is_some())
        );
    }
}
