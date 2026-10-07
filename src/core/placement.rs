use crate::core::disk::MemberDisk;
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementPolicy {
    MostFreeSpace,
    LandingZoneFirst,
}

pub struct PlacementEngine;

impl PlacementEngine {
    /// Selects the best primary disk for a new file.
    /// Excludes disks specified in `exclude_disk_ids`.
    pub fn select_primary_disk<'a>(
        disks: &'a [MemberDisk],
        exclude_disk_ids: &HashSet<String>,
        required_bytes: u64,
        policy: PlacementPolicy,
    ) -> Option<&'a MemberDisk> {
        let eligible: Vec<&'a MemberDisk> = disks
            .iter()
            .filter(|d| d.enabled && !d.read_only)
            .filter(|d| !exclude_disk_ids.contains(&d.id))
            .filter(|d| {
                let stats = d.query_stats();
                stats.online && stats.free_bytes >= required_bytes
            })
            .collect();

        if eligible.is_empty() {
            return None;
        }

        // If landing zone policy is requested and a landing zone disk has enough space:
        if policy == PlacementPolicy::LandingZoneFirst {
            if let Some(lz) = eligible.iter().find(|d| d.is_landing_zone) {
                return Some(lz);
            }
        }

        match policy {
            PlacementPolicy::MostFreeSpace | PlacementPolicy::LandingZoneFirst => {
                eligible.into_iter().max_by_key(|d| d.query_stats().free_bytes)
            }
        }
    }

    /// Selects `target_count` distinct member disks to host replicas of a file.
    /// Invariant: Replicas of the same file are NEVER placed on the same physical drive!
    pub fn select_replica_disks<'a>(
        disks: &'a [MemberDisk],
        existing_disk_ids: &HashSet<String>,
        needed_count: usize,
        file_size: u64,
    ) -> Vec<&'a MemberDisk> {
        let mut selected = Vec::new();
        let mut excluded = existing_disk_ids.clone();

        for _ in 0..needed_count {
            if let Some(disk) = Self::select_primary_disk(
                disks,
                &excluded,
                file_size,
                PlacementPolicy::MostFreeSpace,
            ) {
                excluded.insert(disk.id.clone());
                selected.push(disk);
            } else {
                break;
            }
        }

        selected
    }
}
