//! Lays out the commit graph one row at a time.
//!
//! Each lane holds the commit it expects to see next. A commit takes the
//! leftmost lane waiting for it (or a free one, for a branch tip), lanes that
//! were waiting for it end there, its first parent continues in its lane, and
//! any other parent joins the lane already waiting for it or opens a new one.
//! Rows must arrive children before parents, which a newest-first walk gives.

use neutrino_proto::{GraphRow, GraphSegment};

#[derive(Default)]
pub struct Lanes<Id> {
    /// What each lane is waiting for, and its colour.
    lanes: Vec<Option<(Id, u16)>>,
    next_color: u16,
    pub widest: u16,
}

impl<Id: PartialEq + Clone> Lanes<Id> {
    pub fn new() -> Self {
        Lanes { lanes: Vec::new(), next_color: 0, widest: 0 }
    }

    fn color(&mut self) -> u16 {
        let c = self.next_color;
        self.next_color = self.next_color.wrapping_add(1);
        c
    }

    fn free_lane(&mut self, avoid: Option<usize>) -> usize {
        match self.lanes.iter().enumerate().position(|(i, l)| l.is_none() && Some(i) != avoid) {
            Some(i) => i,
            None => {
                self.lanes.push(None);
                self.lanes.len() - 1
            }
        }
    }

    /// The row for `id`, whose parents are `parents` (first parent first).
    pub fn row(&mut self, id: &Id, parents: &[Id]) -> GraphRow {
        let waiting: Vec<usize> = self.lanes.iter().enumerate().filter(|(_, l)| l.as_ref().is_some_and(|(x, _)| x == id)).map(|(i, _)| i).collect();

        let (lane, color) = match waiting.first() {
            Some(&i) => (i, self.lanes[i].as_ref().unwrap().1),
            None => {
                let i = self.free_lane(None);
                (i, self.color())
            }
        };

        let mut upper = Vec::new();
        for (i, l) in self.lanes.iter().enumerate() {
            if let Some((x, c)) = l {
                let to = if x == id { lane } else { i };
                upper.push(GraphSegment { from: i as u16, to: to as u16, color: *c });
            }
        }
        for &i in &waiting {
            self.lanes[i] = None;
        }

        // Lanes this commit draws into below its dot, and which of them it opened.
        let mut targets: Vec<(usize, u16)> = Vec::new();
        let mut opened: Vec<usize> = Vec::new();
        for (k, p) in parents.iter().enumerate() {
            if let Some(j) = self.lanes.iter().position(|l| l.as_ref().is_some_and(|(x, _)| x == p)) {
                // Someone already waits for this parent: join them.
                let c = self.lanes[j].as_ref().unwrap().1;
                targets.push((j, c));
            } else if k == 0 {
                self.lanes[lane] = Some((p.clone(), color));
                targets.push((lane, color));
                opened.push(lane);
            } else {
                let j = self.free_lane(Some(lane));
                let c = self.color();
                self.lanes[j] = Some((p.clone(), c));
                targets.push((j, c));
                opened.push(j);
            }
        }

        // Every lane that was already open carries on straight down.
        let mut lower = Vec::new();
        for (i, l) in self.lanes.iter().enumerate() {
            if let Some((_, c)) = l {
                if !opened.contains(&i) {
                    lower.push(GraphSegment { from: i as u16, to: i as u16, color: *c });
                }
            }
        }
        for (j, c) in targets {
            lower.push(GraphSegment { from: lane as u16, to: j as u16, color: c });
        }

        while matches!(self.lanes.last(), Some(None)) {
            self.lanes.pop();
        }
        let width = upper.iter().chain(&lower).map(|s| s.from.max(s.to) + 1).max().unwrap_or(1).max(lane as u16 + 1);
        self.widest = self.widest.max(width);

        GraphRow { lane: lane as u16, color, upper, lower }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(v: &[GraphSegment]) -> Vec<(u16, u16)> {
        let mut out: Vec<_> = v.iter().map(|s| (s.from, s.to)).collect();
        out.sort();
        out
    }

    #[test]
    fn straight_line() {
        let mut l = Lanes::new();
        let a = l.row(&3, &[2]);
        let b = l.row(&2, &[1]);
        let c = l.row(&1, &[]);
        assert_eq!((a.lane, b.lane, c.lane), (0, 0, 0));
        assert_eq!(seg(&a.upper), vec![]);
        assert_eq!(seg(&a.lower), vec![(0, 0)]);
        assert_eq!(seg(&b.upper), vec![(0, 0)]);
        assert_eq!(seg(&c.lower), vec![]);
        assert_eq!(l.widest, 1);
    }

    #[test]
    fn merge_and_fork() {
        // M merges F (side branch) into B; both fork from A.
        //   M
        //   |\
        //   B F
        //   |/
        //   A
        let mut l = Lanes::new();
        let m = l.row(&'M', &['B', 'F']);
        assert_eq!(m.lane, 0);
        assert_eq!(seg(&m.lower), vec![(0, 0), (0, 1)]);
        let b = l.row(&'B', &['A']);
        assert_eq!(b.lane, 0);
        assert_eq!(seg(&b.upper), vec![(0, 0), (1, 1)]);
        assert_eq!(seg(&b.lower), vec![(0, 0), (1, 1)]);
        let f = l.row(&'F', &['A']);
        assert_eq!(f.lane, 1);
        // F's parent A is already awaited in lane 0, so F bends into it.
        assert_eq!(seg(&f.lower), vec![(0, 0), (1, 0)]);
        let a = l.row(&'A', &[]);
        assert_eq!(a.lane, 0);
        assert_eq!(seg(&a.upper), vec![(0, 0)]);
        assert_eq!(l.widest, 2);
        assert_ne!(m.lower.iter().find(|s| s.to == 1).unwrap().color, m.color, "the side branch gets its own colour");
    }

    #[test]
    fn two_tips_share_history() {
        // Two branch tips X and Y with the same parent P.
        let mut l = Lanes::new();
        let x = l.row(&'X', &['P']);
        let y = l.row(&'Y', &['P']);
        assert_eq!((x.lane, y.lane), (0, 1));
        assert_eq!(seg(&y.lower), vec![(0, 0), (1, 0)]);
        let p = l.row(&'P', &[]);
        assert_eq!(p.lane, 0);
        assert_eq!(seg(&p.upper), vec![(0, 0)]);
    }

    #[test]
    fn freed_lanes_are_reused() {
        let mut l = Lanes::new();
        l.row(&'M', &['A', 'S']);
        l.row(&'S', &['A']);
        l.row(&'A', &['Z']);
        let n = l.row(&'N', &['Z']);
        assert_eq!(n.lane, 1);
        assert_eq!(l.widest, 2);
    }
}
