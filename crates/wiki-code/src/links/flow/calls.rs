//! Every call of a flow, nested flows included.

use crate::links::model::{Flow, MapCall, Step};
use std::collections::{BTreeMap, BTreeSet};

/// A copy of one enclosing flow's names, for [`Flow::each_call_mut`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowScope {
    pub locals: BTreeSet<String>,
    pub types: BTreeMap<String, String>,
    pub rebound: BTreeSet<String>,
    pub local_class: bool,
    pub inner: BTreeSet<String>,
}

impl Flow {
    /// Every call once, in source order, nested flows included, with the
    /// chain of flows enclosing it, outermost first.
    pub fn each_call<'a>(&'a self, f: &mut impl FnMut(&[&'a Flow], &'a MapCall)) {
        fn walk<'a>(
            steps: &'a [Step],
            chain: &mut Vec<&'a Flow>,
            f: &mut impl FnMut(&[&'a Flow], &'a MapCall),
        ) {
            for s in steps {
                match s {
                    Step::Call(c) => f(chain, c),
                    Step::Branch { arms, .. } => {
                        for a in arms {
                            walk(&a.pre, chain, f);
                            walk(&a.body, chain, f);
                        }
                        for a in arms {
                            walk(&a.after, chain, f);
                        }
                    }
                    Step::Loop {
                        pre,
                        body,
                        post,
                        orelse,
                        ..
                    } => {
                        walk(pre, chain, f);
                        walk(body, chain, f);
                        walk(post, chain, f);
                        walk(orelse, chain, f);
                    }
                    Step::Finally { body, .. } => walk(body, chain, f),
                    Step::Nested { flow, .. } => {
                        chain.push(flow);
                        walk(&flow.steps, chain, f);
                        chain.pop();
                    }
                    Step::Exit { .. } => {}
                }
            }
        }
        walk(&self.steps, &mut vec![self], f);
    }

    /// Every step once, in source order, the steps of nested flows after
    /// the nested step itself.
    pub fn each_step<'a>(&'a self, f: &mut impl FnMut(&'a Step)) {
        fn walk<'a>(steps: &'a [Step], f: &mut impl FnMut(&'a Step)) {
            for s in steps {
                f(s);
                match s {
                    Step::Branch { arms, .. } => {
                        for a in arms {
                            walk(&a.pre, f);
                            walk(&a.body, f);
                            walk(&a.after, f);
                        }
                    }
                    Step::Loop {
                        pre,
                        body,
                        post,
                        orelse,
                        ..
                    } => {
                        for part in [pre, body, post, orelse] {
                            walk(part, f);
                        }
                    }
                    Step::Finally { body, .. } => walk(body, f),
                    Step::Nested { flow, .. } => walk(&flow.steps, f),
                    Step::Call(_) | Step::Exit { .. } => {}
                }
            }
        }
        walk(&self.steps, f);
    }

    /// [`Flow::each_call`] with each call mutable and the chain as copies.
    pub fn each_call_mut(&mut self, f: &mut impl FnMut(&[FlowScope], &mut MapCall)) {
        fn walk(
            steps: &mut [Step],
            chain: &mut Vec<FlowScope>,
            f: &mut impl FnMut(&[FlowScope], &mut MapCall),
        ) {
            for s in steps {
                match s {
                    Step::Call(c) => f(chain, c),
                    Step::Branch { arms, .. } => {
                        for a in arms.iter_mut() {
                            walk(&mut a.pre, chain, f);
                            walk(&mut a.body, chain, f);
                        }
                        for a in arms {
                            walk(&mut a.after, chain, f);
                        }
                    }
                    Step::Loop {
                        pre,
                        body,
                        post,
                        orelse,
                        ..
                    } => {
                        walk(pre, chain, f);
                        walk(body, chain, f);
                        walk(post, chain, f);
                        walk(orelse, chain, f);
                    }
                    Step::Finally { body, .. } => walk(body, chain, f),
                    Step::Nested { flow, .. } => scoped(flow, chain, f),
                    Step::Exit { .. } => {}
                }
            }
        }
        fn scoped(
            flow: &mut Flow,
            chain: &mut Vec<FlowScope>,
            f: &mut impl FnMut(&[FlowScope], &mut MapCall),
        ) {
            chain.push(FlowScope {
                locals: flow.locals.clone(),
                types: flow.types.clone(),
                rebound: flow.rebound.clone(),
                local_class: flow.local_class,
                inner: flow.inner.clone(),
            });
            walk(&mut flow.steps, chain, f);
            chain.pop();
        }
        scoped(self, &mut Vec::new(), f);
    }
}

#[cfg(test)]
mod tests {
    use crate::links::flow::testutil::{call, flow_of, looping};
    use crate::links::model::{Step, Target};

    fn calls(flow: &mut super::Flow) -> Vec<String> {
        let mut seen = Vec::new();
        flow.each_call(&mut |_, c| seen.push(c.site.name.clone()));
        let mut seen_mut = Vec::new();
        flow.each_call_mut(&mut |_, c| {
            seen_mut.push(c.site.name.clone());
            c.target = Target::Outside;
        });
        assert_eq!(seen, seen_mut);
        seen
    }

    #[test]
    fn a_loops_post_is_visited_after_its_body() {
        let mut flow = flow_of(vec![looping(
            "for",
            vec![call("a")],
            vec![call("u")],
            false,
        )]);
        assert_eq!(calls(&mut flow), ["a", "u"]);
        let Step::Loop { post, .. } = &flow.steps[0] else {
            unreachable!()
        };
        let Step::Call(u) = &post[0] else {
            unreachable!()
        };
        assert_eq!(u.target, Target::Outside);
    }
}
