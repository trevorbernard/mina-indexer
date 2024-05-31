use crate::block::{
    extract_block_height, extract_block_height_or_max, extract_state_hash, previous_state_hash::*,
};
use log::{debug, info};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::Instant,
};

/// Separate blocks into 3 length-sorted lists:
/// - deep canonical blocks (canonical up to root)
/// - recent blocks (following the witness tree root)
/// - orphaned blocks (at or below witness tree root)
pub fn discovery(
    min_len_filter: Option<u32>,
    max_len_filter: Option<u32>,
    canonical_threshold: u32,
    reporting_freq: u32,
    mut paths: Vec<&PathBuf>,
) -> anyhow::Result<(Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>)> {
    let mut deep_canonical_state_hashes = HashSet::new();
    let mut deep_canonical_paths = vec![];
    let mut recent_paths = vec![];

    if !paths.is_empty() {
        info!("Sorting precomputed blocks by length");

        let time = Instant::now();
        paths.sort_by_cached_key(|x| extract_block_height_or_max(x));

        info!(
            "{} blocks sorted by length in {:?}",
            paths.len() + 1, // +1 genesis
            time.elapsed(),
        );

        if let Some(blockchain_length) = max_len_filter {
            debug!("Applying max length block filter: blockchain_length < {blockchain_length}");
            paths.retain(|p| extract_block_height_or_max(p) <= blockchain_length);
        }

        if let Some(blockchain_length) = min_len_filter {
            debug!("Applying min length block filter: blockchain_length > {blockchain_length}");
            paths.retain(|p| extract_block_height_or_max(p) >= blockchain_length);
        }

        if paths.is_empty() {
            return Ok((vec![], vec![], vec![]));
        }

        // keep track of:
        // - diffs between blocks of recent lengths (to find gaps)
        // - starting index for each collection of blocks of a fixed length
        // - length of the current path under investigation

        let mut length_start_indices_and_diffs = vec![];
        // paths will always have at least 1 item
        let mut curr_length = extract_block_height_or_max(paths.first().unwrap());

        info!("Searching for deep canonical blocks in blocks directory");
        for (idx, path) in paths.iter().enumerate() {
            let length = extract_block_height_or_max(path);
            if length > curr_length || idx == 0 {
                length_start_indices_and_diffs.push((idx, length - curr_length));
                curr_length = length;
            }
        }

        // check that there are enough contiguous blocks for a canonical chain
        let last_contiguous_first_noncontiguous_start_idx =
            last_contiguous_first_noncontiguous_start_idx(&length_start_indices_and_diffs);
        let last_contiguous_start_idx = last_contiguous_first_noncontiguous_start_idx
            .map(|i| i.0)
            .unwrap_or(length_start_indices_and_diffs.last().unwrap().0);
        let last_contiguous_idx = last_contiguous_first_noncontiguous_start_idx
            .map(|i| i.1.saturating_sub(1))
            .unwrap_or(paths.len() - 1);
        let witness_tree_root_opt = find_witness_tree_root(
            paths.as_slice(),
            &length_start_indices_and_diffs,
            length_start_indices_and_diffs
                .iter()
                .position(|x| x.0 == last_contiguous_start_idx)
                .unwrap_or(0),
            last_contiguous_idx,
            canonical_threshold,
        );

        if witness_tree_root_opt.is_none()
            || max_num_canonical_blocks(&length_start_indices_and_diffs, last_contiguous_start_idx)
                < canonical_threshold
        {
            info!(
                "No deep canoncial blocks were found other than genesis. Adding all blocks to the witness tree."
            );
            return Ok((vec![], paths.into_iter().cloned().collect(), vec![]));
        }

        // backtrack `MAINNET_CANONICAL_THRESHOLD` blocks from
        // the `last_contiguous_idx` to find the witness tree root
        let (mut curr_length_idx, mut curr_start_idx) = witness_tree_root_opt.unwrap();
        let mut curr_path = paths[curr_length_idx];

        info!(
            "Found witness tree root (length {}): {}",
            extract_block_height(curr_path).unwrap_or(0),
            extract_state_hash(curr_path),
        );

        // handle all blocks that are higher than the witness tree root
        if let Some(recent_start_idx) = next_length_start_index(paths.as_slice(), curr_length_idx) {
            if recent_start_idx
                < length_start_indices_and_diffs
                    .last()
                    .map(|(idx, _)| *idx)
                    .unwrap_or(0)
            {
                for path in paths[recent_start_idx..].iter() {
                    recent_paths.push(path.to_path_buf());
                }
            }
        }

        // collect the deep canonical blocks
        deep_canonical_paths.push(curr_path.clone());
        deep_canonical_state_hashes.insert(extract_state_hash(curr_path));

        if deep_canonical_paths.len() < reporting_freq as usize {
            info!("Walking the canonical chain back to genesis");
        } else {
            info!(
                "Walking the canonical chain back to genesis, reporting every {} blocks",
                reporting_freq
            );
        }

        let time = Instant::now();
        let mut count = 1;

        // descend from the witness tree root to the lowest block in the dir,
        // segment by segment, searching for ancestors
        while curr_start_idx > 0 {
            if count % reporting_freq == 0 {
                info!(
                    "Found {} deep canonical blocks in {:?}",
                    count,
                    time.elapsed()
                );
            }

            // search for parent in previous segment's blocks
            let mut parent_found = false;
            let prev_length_idx = length_start_indices_and_diffs[curr_start_idx - 1].0;
            let parent_hash = PreviousStateHash::from_path(curr_path)?.0;

            for path in paths[prev_length_idx..curr_length_idx].iter() {
                if parent_hash == extract_state_hash(path) {
                    deep_canonical_paths.push(path.to_path_buf());
                    deep_canonical_state_hashes.insert(extract_state_hash(path));
                    curr_path = path;
                    curr_length_idx = prev_length_idx;
                    count += 1;
                    curr_start_idx -= 1;
                    parent_found = true;
                    break;
                }
            }

            // handle case where we fail to find parent
            if !parent_found {
                info!(
                    "Unable to locate parent block: mainnet-{}-{parent_hash}.json",
                    extract_block_height_or_max(curr_path) - 1,
                );
                return Ok((vec![], paths.into_iter().cloned().collect(), vec![]));
            }
        }

        // push the lowest canonical block
        for path in paths[..curr_length_idx].iter() {
            let prev_hash = PreviousStateHash::from_path(curr_path)?.0;
            if prev_hash == extract_state_hash(path) {
                debug!("Lowest canonical block found");
                deep_canonical_paths.push(path.to_path_buf());
                deep_canonical_state_hashes.insert(extract_state_hash(path));
                break;
            }
        }

        // sort lowest to highest
        deep_canonical_paths.reverse();

        info!(
            "Found {} blocks in the canonical chain in {:?}",
            deep_canonical_paths.len() as u32 + 1 + canonical_threshold,
            time.elapsed(),
        );
    }

    let max_canonical_length = deep_canonical_paths
        .last()
        .and_then(|p| extract_block_height(p))
        .unwrap_or(1);
    let orphaned_paths: Vec<PathBuf> = paths
        .into_iter()
        .filter(|p| {
            if let Some(length) = extract_block_height(p) {
                return length <= max_canonical_length
                    && !deep_canonical_state_hashes.contains(&extract_state_hash(p));
            }
            false
        })
        .cloned()
        .collect();

    Ok((
        deep_canonical_paths.to_vec(),
        recent_paths.to_vec(),
        orphaned_paths,
    ))
}

/// Checks if the block at `curr_path` is the _parent_ of the block at `path`.
fn is_parent(path: &Path, curr_path: &Path) -> bool {
    if let Ok(prev_hash) = PreviousStateHash::from_path(curr_path) {
        let prev_hash: String = prev_hash.into();
        return prev_hash == extract_state_hash(path);
    }
    false
}

/// Returns the start index of the paths with next higher length.
fn next_length_start_index(paths: &[&PathBuf], path_idx: usize) -> Option<usize> {
    let length = extract_block_height_or_max(paths[path_idx]);
    for (n, path) in paths[path_idx..].iter().enumerate() {
        if extract_block_height_or_max(path) > length {
            return Some(path_idx + n);
        }
    }
    None
}

/// Finds the root of the witness tree, i.e. the _highest_ block in the
/// _lowest contiguous chain_ with `canonical_threshold` ancestors.
/// Unfortunately, the existence of this value does not necessarily imply
/// the existence of a canonical chain within the collection of blocks.
///
/// Returns the index of the witness tree root in `paths` and
/// the start index of the first "recent" block (these blocks are added to the
/// witness tree, on top of the root).
fn find_witness_tree_root(
    paths: &[&PathBuf],
    length_start_indices_and_diffs: &[(usize, u32)],
    mut curr_start_idx: usize,
    mut curr_length_idx: usize,
    canonical_threshold: u32,
) -> Option<(usize, usize)> {
    if length_start_indices_and_diffs.len() <= canonical_threshold as usize {
        None
    } else {
        let mut offset = 0;
        let mut curr_path = &paths[curr_length_idx];

        for n in 1..=canonical_threshold {
            let mut parent_found = false;
            let prev_length_start_idx = if curr_start_idx > 0 {
                length_start_indices_and_diffs[curr_start_idx - 1].0
            } else {
                0
            };

            for (idx, path) in paths[prev_length_start_idx..curr_length_idx]
                .iter()
                .enumerate()
            {
                // if the parent is found, check that it has a parent, etc
                if is_parent(path, curr_path) {
                    debug!(
                        "{} is the parent of {}",
                        curr_path
                            .file_stem()
                            .unwrap_or_default()
                            .to_str()
                            .unwrap_or_default(),
                        path.file_stem()
                            .unwrap_or_default()
                            .to_str()
                            .unwrap_or_default(),
                    );
                    offset = idx;
                    curr_path = path;
                    curr_length_idx = prev_length_start_idx;
                    curr_start_idx = curr_start_idx.saturating_sub(1);
                    parent_found = true;
                    continue;
                }
            }

            // if a parent was not found
            if !parent_found {
                // begin the search again at the previous length
                if curr_start_idx > canonical_threshold as usize {
                    return find_witness_tree_root(
                        paths,
                        length_start_indices_and_diffs,
                        curr_start_idx.saturating_sub(1),
                        prev_length_start_idx,
                        canonical_threshold,
                    );
                } else {
                    // root cannot be found
                    return None;
                }
            }

            // potential root found
            if n == canonical_threshold && parent_found {
                break;
            }
        }
        Some((curr_length_idx + offset, curr_start_idx))
    }
}

/// Finds the index of the _highest possible block in the lowest contiguous
/// chain_ and the starting index of the next higher blocks.
fn last_contiguous_first_noncontiguous_start_idx(
    length_start_indices_and_diffs: &[(usize, u32)],
) -> Option<(usize, usize)> {
    let mut prev = 0;
    for (idx, diff) in length_start_indices_and_diffs.iter() {
        if *diff > 1 {
            return Some((prev, *idx));
        } else {
            prev = *idx;
        }
    }
    None
}

fn max_num_canonical_blocks(
    length_start_indices_and_diffs: &[(usize, u32)],
    last_contiguous_start_idx: usize,
) -> u32 {
    length_start_indices_and_diffs
        .iter()
        .position(|x| x.0 == last_contiguous_start_idx)
        .unwrap_or(0) as u32
        + 1
}
