// Which heap nodes (`Box<T>` cells and `List<T>` buffers) can reach each other when they are dropped.
// A node that lies on a cycle of this graph (mutually recursive boxes, a list of nodes that hold lists, ...)
// is dropped with an explicit worklist instead of recursion, in both the IR glue (ir.rs) and the C
// backend (cgen.rs), so any depth of nesting is released with constant stack. Dynamic indirections
// (closures, `dyn` objects, shared cells) are not nodes: see docs/BACKEND.md for what they do.
use crate::syntax::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum HeapNode {
    Box(usize),
    List(usize),
}

impl Types {
    /// A struct whose drop is the plain field-by-field drop (no guard, handle, shared cell or closure environment).
    pub fn plain_struct_drop(&self, sid: usize) -> bool {
        !self.is_lock_guard(sid)
            && self.join_payload(sid).is_none()
            && self.runtime_struct(sid, "Shared").is_none()
            && !self.structs[sid].cname.starts_with("Env__")
    }

    /// The heap nodes that dropping a value of type `ty` reaches directly: through struct fields and enum
    /// payloads, not through another heap node.
    pub fn direct_nodes(&self, ty: Ty, out: &mut Vec<HeapNode>) {
        match ty {
            Ty::Box(b) => out.push(HeapNode::Box(b)),
            Ty::List(l) if self.owned(self.lists[l]) => out.push(HeapNode::List(l)),
            Ty::Struct(sid) if self.plain_struct_drop(sid) => {
                for (_, f) in &self.structs[sid].fields {
                    self.direct_nodes(*f, out);
                }
            }
            Ty::Enum(eid) => {
                for (_, ps) in &self.enums[eid].variants {
                    for p in ps {
                        self.direct_nodes(*p, out);
                    }
                }
            }
            _ => {}
        }
    }

    pub fn node_content(&self, n: HeapNode) -> Ty {
        match n {
            HeapNode::Box(b) => self.boxes[b],
            HeapNode::List(l) => self.lists[l],
        }
    }

    fn node_succs(&self, n: HeapNode) -> Vec<HeapNode> {
        let mut v = vec![];
        self.direct_nodes(self.node_content(n), &mut v);
        v.sort();
        v.dedup();
        v
    }

    /// Does dropping a value of `ty` reach one of `members` directly?
    pub fn reaches_node(&self, ty: Ty, members: &[HeapNode]) -> bool {
        let mut v = vec![];
        self.direct_nodes(ty, &mut v);
        v.iter().any(|n| members.contains(n))
    }

    fn reach_from(&self, n: HeapNode) -> Vec<HeapNode> {
        let mut seen: Vec<HeapNode> = vec![];
        let mut stack = self.node_succs(n);
        while let Some(x) = stack.pop() {
            if seen.contains(&x) {
                continue;
            }
            seen.push(x);
            stack.extend(self.node_succs(x));
        }
        seen
    }

    /// The nodes on a cycle with `n` (sorted, `n` included), or `None` when `n` is not recursive.
    pub fn recursive_scc(&self, n: HeapNode) -> Option<Vec<HeapNode>> {
        if self.has_param(match n {
            HeapNode::Box(b) => Ty::Box(b),
            HeapNode::List(l) => Ty::List(l),
        }) {
            return None;
        }
        let r = self.reach_from(n);
        if !r.contains(&n) {
            return None;
        }
        let mut m: Vec<HeapNode> = r
            .into_iter()
            .filter(|x| *x == n || self.reach_from(*x).contains(&n))
            .collect();
        m.sort();
        m.dedup();
        Some(m)
    }

    /// The component to drop with the general worklist: every recursive node except a box that only reaches
    /// itself (that one keeps its cheaper chain/tree loop, see ir.rs `drop_box_iterative`).
    pub fn general_scc(&self, n: HeapNode) -> Option<Vec<HeapNode>> {
        let m = self.recursive_scc(n)?;
        if m.len() == 1 && matches!(m[0], HeapNode::Box(_)) {
            return None;
        }
        Some(m)
    }
}
