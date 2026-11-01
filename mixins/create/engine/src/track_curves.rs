//! The curved track joined to track blocks (`TrackBlockEntity.connections`, `BezierConnection`
//! as `TrackPlacement.placeTracks` adds it): each curve is kept at both of its track blocks, the
//! first one's copy primary (it draws the curve), the other's the same curve reversed.

use crate::bezier::{Curve, Vec3};
use std::collections::HashMap;

/// One end's view of a curve: the track block at its other end and the curve from here.
#[derive(Clone, Debug, PartialEq)]
pub struct Connection {
    pub to: [i32; 3],
    pub curve: Curve,
    /// `isPrimary`: the copy that draws it.
    pub primary: bool,
}

/// What a curve is made of, as saved and sent: its ends' blocks, starts, axes and normals.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Saved {
    pub from: [i32; 3],
    pub to: [i32; 3],
    pub starts: [Vec3; 2],
    pub axes: [Vec3; 2],
    pub normals: [Vec3; 2],
}

impl Saved {
    pub fn curve(&self) -> Curve {
        Curve::new(self.starts, self.axes, self.normals)
    }

    /// The numbers in a row: 6 ints, then 18 floats.
    pub fn numbers(&self) -> ([i32; 6], [f64; 18]) {
        let mut f = [0.0; 18];
        let mut k = 0;
        for set in [self.starts, self.axes, self.normals] {
            for v in set {
                f[k..k + 3].copy_from_slice(&v);
                k += 3;
            }
        }
        let (a, b) = (self.from, self.to);
        ([a[0], a[1], a[2], b[0], b[1], b[2]], f)
    }

    pub fn from_numbers(i: [i32; 6], f: [f64; 18]) -> Saved {
        let v = |k: usize| [f[k], f[k + 1], f[k + 2]];
        Saved {
            from: [i[0], i[1], i[2]],
            to: [i[3], i[4], i[5]],
            starts: [v(0), v(3)],
            axes: [v(6), v(9)],
            normals: [v(12), v(15)],
        }
    }
}

/// A dimension's curves by track block, and the blocks whose curves changed since clients were
/// told.
#[derive(Debug, Default)]
pub struct Curves {
    pub at: HashMap<[i32; 3], Vec<Connection>>,
    pub changed: Vec<[i32; 3]>,
    /// Counts every change, for the track graph to know it must be built again.
    pub revision: u64,
}

impl Curves {
    /// `addConnection` on both ends: `from`'s copy primary, `to`'s the reverse.
    pub fn add(&mut self, s: Saved) {
        let curve = s.curve();
        let back = Curve::new(
            [s.starts[1], s.starts[0]],
            [s.axes[1], s.axes[0]],
            [s.normals[1], s.normals[0]],
        );
        for (here, there, curve, primary) in
            [(s.from, s.to, curve, true), (s.to, s.from, back, false)]
        {
            let list = self.at.entry(here).or_default();
            list.retain(|c| c.to != there);
            list.push(Connection {
                to: there,
                curve,
                primary,
            });
        }
        self.changed.push(s.from);
        self.changed.push(s.to);
        self.revision += 1;
    }

    /// The track at `pos` is gone (`TrackBlockEntity.removeInboundConnections`): its curves
    /// and their other ends' copies go too; the other ends, for the caller to update.
    pub fn remove_at(&mut self, pos: [i32; 3]) -> Vec<[i32; 3]> {
        let Some(list) = self.at.remove(&pos) else {
            return Vec::new();
        };
        let others: Vec<[i32; 3]> = list.iter().map(|c| c.to).collect();
        for o in &others {
            if let Some(l) = self.at.get_mut(o) {
                l.retain(|c| c.to != pos);
                if l.is_empty() {
                    self.at.remove(o);
                }
            }
        }
        self.changed.push(pos);
        self.revision += 1;
        self.changed.extend(&others);
        others
    }

    /// The curves whose primary end is in column (cx, cz), to save with it.
    pub fn column(&self, cx: i32, cz: i32) -> Vec<Saved> {
        let mut out: Vec<Saved> = self
            .at
            .iter()
            .filter(|(p, _)| (p[0] >> 4, p[2] >> 4) == (cx, cz))
            .flat_map(|(p, list)| {
                list.iter().filter(|c| c.primary).map(move |c| Saved {
                    from: *p,
                    to: c.to,
                    starts: c.curve.starts,
                    axes: c.curve.axes,
                    normals: c.curve.normals,
                })
            })
            .collect();
        out.sort_by_key(|s| (s.from, s.to));
        out
    }

    /// The primary curves drawn from `pos`.
    pub fn drawn_from(&self, pos: [i32; 3]) -> Vec<Saved> {
        self.at.get(&pos).map_or_else(Vec::new, |list| {
            list.iter()
                .filter(|c| c.primary)
                .map(|c| Saved {
                    from: pos,
                    to: c.to,
                    starts: c.curve.starts,
                    axes: c.curve.axes,
                    normals: c.curve.normals,
                })
                .collect()
        })
    }
}

/// The mod message channel for curves, server to client.
pub const CHANNEL: &str = "create:track_curves";

/// A block's primary curves as clients get them: the block, then each curve's numbers; a block
/// with none clears what it drew.
pub fn encode(list: &[([i32; 3], Vec<Saved>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (pos, curves) in list {
        for v in pos {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.push(curves.len().min(255) as u8);
        for s in curves.iter().take(255) {
            let (i, f) = s.numbers();
            for v in i {
                out.extend_from_slice(&v.to_le_bytes());
            }
            for v in f {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    out
}

pub fn decode(mut data: &[u8]) -> Vec<([i32; 3], Vec<Saved>)> {
    let mut out = Vec::new();
    let int = |d: &[u8], k: usize| i32::from_le_bytes([d[k], d[k + 1], d[k + 2], d[k + 3]]);
    while data.len() >= 13 {
        let pos = [int(data, 0), int(data, 4), int(data, 8)];
        let n = usize::from(data[12]);
        data = &data[13..];
        let mut curves = Vec::with_capacity(n);
        for _ in 0..n {
            if data.len() < 24 + 144 {
                return out;
            }
            let i: [i32; 6] = std::array::from_fn(|k| int(data, 4 * k));
            let f: [f64; 18] = std::array::from_fn(|k| {
                let b = &data[24 + 8 * k..32 + 8 * k];
                f64::from_le_bytes(b.try_into().unwrap_or([0; 8]))
            });
            curves.push(Saved::from_numbers(i, f));
            data = &data[24 + 144..];
        }
        out.push((pos, curves));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const UP: Vec3 = [0.0, 1.0, 0.0];

    fn turn() -> Saved {
        Saved {
            from: [0, 0, 0],
            to: [8, 0, 8],
            starts: [[0.5, 0.0, 0.5], [8.5, 0.0, 8.5]],
            axes: [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0]],
            normals: [UP, UP],
        }
    }

    #[test]
    fn a_curve_sits_at_both_ends_and_goes_with_either() {
        let mut c = Curves::default();
        c.add(turn());
        assert_eq!(c.at[&[0, 0, 0]].len(), 1);
        assert!(c.at[&[0, 0, 0]][0].primary && !c.at[&[8, 0, 8]][0].primary);
        // The reverse runs the same path the other way.
        let back = &c.at[&[8, 0, 8]][0].curve;
        let fwd = &c.at[&[0, 0, 0]][0].curve;
        let (a, b) = (fwd.position(0.25), back.position(0.75));
        assert!((0..3).all(|k| (a[k] - b[k]).abs() < 1e-6), "{a:?} {b:?}");
        // Saved with the first end's column only, and read back the same.
        assert_eq!(c.column(0, 0), vec![turn()]);
        let (i, f) = turn().numbers();
        assert_eq!(Saved::from_numbers(i, f), turn());
        let sent = encode(&[([0, 0, 0], c.drawn_from([0, 0, 0]))]);
        assert_eq!(decode(&sent), vec![([0, 0, 0], vec![turn()])]);
        // Breaking the far end takes it from both.
        assert_eq!(c.remove_at([8, 0, 8]), vec![[0, 0, 0]]);
        assert!(c.at.is_empty());
    }
}
