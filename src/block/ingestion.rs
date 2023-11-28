use crossbeam_channel::bounded;
use glob::glob;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use tracing::{debug, error, info, trace, warn};

/// Deterimines if the filename is a valid precomputed block
///
/// Precomputed blocks uploaded to the o1 gcloud bucket has two valid filename patterns:
/// 1. `<network>-<block-height>-<state-hash>.json`
/// 2. `<network>-<state-hash>.json`
///
/// Check that the filename matches one of the two patterns and that the `block-height` (if exists) can be evaluated to be a number and that the `state-hash` starts with `3N`
pub fn is_valid_block_file(path: &Path) -> bool {
    if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
        let parts: Vec<&str> = file_name.split('-').collect();
        match parts.len() {
            2 => parts[1].ends_with(".json") && parts[1].starts_with("3N"),
            3 => {
                parts[1].parse::<u64>().is_ok()
                    && parts[2].ends_with(".json")
                    && parts[2].starts_with("3N")
            }
            _ => false,
        }
    } else {
        false
    }
}

/// Take a directory of precomputed blocks and returning them in ascending block height order
///
/// Simple heuristic to build the blockchain in order without finding the canonical chain first
pub fn find_and_sort_json_blocks(blocks_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut paths: Vec<(u64, PathBuf)> = glob(&format!("{}/*.json", blocks_dir.display()))?
        .filter_map(|x| x.ok())
        .filter_map(|path| {
            if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
                let parts: Vec<&str> = file_name.split('-').collect();
                if parts.len() == 3 && parts[2].ends_with(".json") {
                    parts[1]
                        .parse::<u64>()
                        .ok()
                        .map(|block_height| (block_height, path))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();
    paths.sort_by_key(|k| k.0);
    Ok(paths.into_iter().map(|(_, path)| path).collect())
}

/// Comsumes a directory of precomputed blocks ensuring their validity and block height order and sends it downstream
pub fn consume_directory_for_blocks(
    blocks_dir: &Path,
    sender: crossbeam_channel::Sender<PathBuf>,
) -> anyhow::Result<()> {
    for path in find_and_sort_json_blocks(blocks_dir)? {
        if let Err(e) = sender.send(path) {
            error!("Unable to send path downstream. {}", e);
        }
    }
    Ok(())
}

/// Watches a directory listening for when valid precomputed blocks are created and signals downstream
pub fn watch_directory_for_blocks<P: AsRef<Path>>(
    watch_dir: P,
    sender: crossbeam_channel::Sender<PathBuf>,
) -> notify::Result<()> {
    let (tx, rx) = bounded(4096);
    let mut watcher = RecommendedWatcher::new(tx, Config::default())?;

    watcher.watch(watch_dir.as_ref(), RecursiveMode::NonRecursive)?;
    info!("Listening for precomputed blocks: {:?}", watch_dir.as_ref());
    for res in rx {
        match res {
            Ok(event) => {
                if let EventKind::Create(notify::event::CreateKind::File) = event.kind {
                    for path in event.paths {
                        trace!("File Created Signal");
                        if is_valid_block_file(&path) {
                            debug!("Valid precomputed block file");
                            if let Err(e) = sender.send(path) {
                                error!("Unable to send path downstream. {}", e);
                            }
                        } else {
                            warn!("Invalid precomputed block file: {}", path.display());
                        }
                    }
                }
            }
            Err(error) => error!("Error: {error:?}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fmt,
        path::{Path, PathBuf},
        thread,
    };

    use crossbeam_channel::bounded;
    use quickcheck::{Arbitrary, Gen};
    use quickcheck_macros::quickcheck;

    use crate::block::ingestion::is_valid_block_file;

    use super::{consume_directory_for_blocks, find_and_sort_json_blocks};

    #[derive(Debug, Clone)]
    struct BlockFileName(PathBuf);

    #[derive(Debug, Clone)]
    enum Network {
        MAINNET,
        DEVNET,
        TESTWORLD,
        BERKELEY,
    }

    impl Arbitrary for Network {
        fn arbitrary(g: &mut Gen) -> Self {
            let idx = usize::arbitrary(g) % 4;
            match idx {
                0 => Network::MAINNET,
                1 => Network::DEVNET,
                2 => Network::TESTWORLD,
                3 => Network::BERKELEY,
                _ => panic!("should never happen"),
            }
        }
    }

    impl fmt::Display for Network {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let network = match self {
                Network::MAINNET => "mainnet",
                Network::DEVNET => "devnet",
                Network::TESTWORLD => "testworld",
                Network::BERKELEY => "berkeley",
            };
            write!(f, "{}", network)
        }
    }

    impl Arbitrary for BlockFileName {
        fn arbitrary(g: &mut Gen) -> BlockFileName {
            let network = Network::arbitrary(g);
            let height = u64::arbitrary(g);
            let hash = "3N";
            let is_first_pattern = bool::arbitrary(g);
            let path = if is_first_pattern {
                PathBuf::from(&format!("{}-{}-{}.json", network, height, hash))
            } else {
                PathBuf::from(&format!("{}-{}.json", network, hash))
            };
            Self(path)
        }
    }

    #[quickcheck]
    fn check_for_block_file_validity(valid_block: BlockFileName) -> bool {
        is_valid_block_file(valid_block.0.as_path())
    }

    #[test]
    fn test_find_and_sort_json_blocks() -> anyhow::Result<()> {
        let path = PathBuf::from("./tests/data/canonical_chain_discovery/contiguous");
        let actual: Vec<String> = find_and_sort_json_blocks(&path)?
            .into_iter()
            .filter_map(|p| p.into_os_string().into_string().ok())
            .collect();
        let expected: Vec<&str> = vec![
            "tests/data/canonical_chain_discovery/contiguous/mainnet-2-3NLyWnjZqUECniE1q719CoLmes6WDQAod4vrTeLfN7XXJbHv6EHH.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-3-3NKd5So3VNqGZtRZiWsti4yaEe1fX79yz5TbfG6jBZqgMnCQQp3R.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-4-3NL9qBsNibXPm5Nh8cSg5CCqrbzX5VUVY9gJzAbg7EVCF3hfhazG.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-5-3NKQUoBfi9vkbuqtDJmSEYBQrcSo4GjwG8bPCiii4yqM8AxEQvtY.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-6-3NKqRR2BZFV7Ad5kxtGKNNL59neXohf4ZEC5EMKrrnijB1jy4R5v.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-7-3NLGcwFVQF1p1PrZpusw2fZwBe5HKXGtrGy1Vc4aPkeBtT8nMNUc.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-8-3NLVZQz4FwFbvW4hejfyRpw5NyP8XvQjhj4wSsCjCKdHNBjwWsPG.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-9-3NKknQGpDQu6Afe1VYuHYbEfnjbHT3xGZaFCd8sueL8CoJkx5kPw.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-10-3NKGgTk7en3347KH81yDra876GPAUSoSePrfVKPmwR1KHfMpvJC5.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-11-3NLMeYAFXxsmhSFtLHFxdtjGcfHTVFmBmBF8uTJvP4Ve5yEmxYeA.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-12-3NKkJDmNZGYdKVDDJkkamGdvNzASia2SXxKpu18imps7KqbNXENY.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-13-3NKXzc1hAE1bK9BSkJUhBBSznMhwW3ZxUTgdoLoqzW6SvqVFcAw5.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-14-3NKDTKbWye6GcdjRu28sSSUgwkNDZXZJvsVZpXAR4YeawhYLqjtE.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-15-3NKkVW47d5Zxi7zvKufBrbiAvLzyKnFgsnN9vgCw65sffvHpv63M.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-16-3NL1sy75LXQScPZda2ywNmdVPiJDnYFe5wV7YLzyRcPVgmDkemW9.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-17-3NKDWsSnHUHN6iakRuBY4LcNou8ToQ3jHpMWkyp6gposjjXC6XUu.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-18-3NLZhhUTMGiWe9UYxY8aYHvRVSoKJTHgKJvopBdC2RA9KisGfPuo.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-19-3NLEu5K5pmEH1CSKZJd94eJatDTM3djoeJTVE3RkcNztJ4z63bM6.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-20-3NLPpt5SyVnD1U5uJAqR3DL1Cqj5dG26SuWutRQ6AQpbQtQUWSYA.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-21-3NKZ6DTHiMtuaeP3tJq2xe4uujVRnGT9FX1rBiZY521uNToSppUZ.json"
        ];
        assert_eq!(
            expected, actual,
            "Directory should be in ascending order by block height"
        );
        Ok(())
    }

    #[test]
    fn test_consume_directory_for_blocks_contiguous() -> anyhow::Result<()> {
        let (tx, rx) = bounded(64);

        let blocks_dir = Path::new("./tests/data/canonical_chain_discovery/contiguous");
        // Thread sends PathBufs downstream via the sender
        let handle = thread::spawn(move || {
            let _ = consume_directory_for_blocks(blocks_dir, tx);
        });
        // Wait for thread to finish
        handle.join().expect("Should be able to join on handle");
        // Aggregate all the PathBufs from the Receiver part of the channel
        let actual: Vec<String> = rx
            .into_iter()
            .filter_map(|p| p.into_os_string().into_string().ok())
            .collect();
        let expected: Vec<&str> = vec![
            "tests/data/canonical_chain_discovery/contiguous/mainnet-2-3NLyWnjZqUECniE1q719CoLmes6WDQAod4vrTeLfN7XXJbHv6EHH.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-3-3NKd5So3VNqGZtRZiWsti4yaEe1fX79yz5TbfG6jBZqgMnCQQp3R.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-4-3NL9qBsNibXPm5Nh8cSg5CCqrbzX5VUVY9gJzAbg7EVCF3hfhazG.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-5-3NKQUoBfi9vkbuqtDJmSEYBQrcSo4GjwG8bPCiii4yqM8AxEQvtY.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-6-3NKqRR2BZFV7Ad5kxtGKNNL59neXohf4ZEC5EMKrrnijB1jy4R5v.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-7-3NLGcwFVQF1p1PrZpusw2fZwBe5HKXGtrGy1Vc4aPkeBtT8nMNUc.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-8-3NLVZQz4FwFbvW4hejfyRpw5NyP8XvQjhj4wSsCjCKdHNBjwWsPG.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-9-3NKknQGpDQu6Afe1VYuHYbEfnjbHT3xGZaFCd8sueL8CoJkx5kPw.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-10-3NKGgTk7en3347KH81yDra876GPAUSoSePrfVKPmwR1KHfMpvJC5.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-11-3NLMeYAFXxsmhSFtLHFxdtjGcfHTVFmBmBF8uTJvP4Ve5yEmxYeA.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-12-3NKkJDmNZGYdKVDDJkkamGdvNzASia2SXxKpu18imps7KqbNXENY.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-13-3NKXzc1hAE1bK9BSkJUhBBSznMhwW3ZxUTgdoLoqzW6SvqVFcAw5.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-14-3NKDTKbWye6GcdjRu28sSSUgwkNDZXZJvsVZpXAR4YeawhYLqjtE.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-15-3NKkVW47d5Zxi7zvKufBrbiAvLzyKnFgsnN9vgCw65sffvHpv63M.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-16-3NL1sy75LXQScPZda2ywNmdVPiJDnYFe5wV7YLzyRcPVgmDkemW9.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-17-3NKDWsSnHUHN6iakRuBY4LcNou8ToQ3jHpMWkyp6gposjjXC6XUu.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-18-3NLZhhUTMGiWe9UYxY8aYHvRVSoKJTHgKJvopBdC2RA9KisGfPuo.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-19-3NLEu5K5pmEH1CSKZJd94eJatDTM3djoeJTVE3RkcNztJ4z63bM6.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-20-3NLPpt5SyVnD1U5uJAqR3DL1Cqj5dG26SuWutRQ6AQpbQtQUWSYA.json", "tests/data/canonical_chain_discovery/contiguous/mainnet-21-3NKZ6DTHiMtuaeP3tJq2xe4uujVRnGT9FX1rBiZY521uNToSppUZ.json"
        ];
        assert_eq!(expected, actual, "Results should be in ascending order by block height");
        Ok(())
    }
}
