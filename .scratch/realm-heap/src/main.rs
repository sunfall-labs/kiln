//! Bounded feasibility probe for a Rust-side realm graph.
//!
//! This is intentionally independent of the workspace: it tests only the two properties the
//! proposed collector would have to provide before Kiln rewrites `Value`: exact tracing of a
//! cycle and a root that can outlive one mutation callback.  It does not claim to model the
//! interpreter or the native ABI.

use gc_arena::{Arena, Collect, DynamicRootSet, Gc, GcWeak, Mutation, RefLock, Rootable};

#[derive(Copy, Clone, Collect)]
#[collect(no_drop)]
struct Node<'gc> {
    next: Option<Gc<'gc, RefLock<Node<'gc>>>>,
    payload: u64,
}

type NodePtr<'gc> = Gc<'gc, RefLock<Node<'gc>>>;

#[derive(Collect)]
#[collect(no_drop)]
struct Roots<'gc> {
    live: NodePtr<'gc>,
    weak_unreachable: GcWeak<'gc, RefLock<Node<'gc>>>,
}

type HeldRoot = Rootable![NodePtr<'_>];

fn node<'gc>(mc: &Mutation<'gc>, payload: u64) -> NodePtr<'gc> {
    Gc::new(
        mc,
        RefLock::new(Node {
            next: None,
            payload,
        }),
    )
}

fn main() {
    // A cycle is legal in the graph and must not keep an unreachable island alive forever.
    let mut arena = Arena::<Rootable![Roots<'_>]>::new(|mc| {
        let root = node(mc, 1);
        let two = node(mc, 2);
        let unreachable = node(mc, 99);
        root.borrow_mut(mc).next = Some(two);
        two.borrow_mut(mc).next = Some(root);
        // No strong root points at this node.  Keeping only a weak pointer lets the test observe
        // that sweep actually reclaimed it without accidentally rooting it.
        Roots {
            live: root,
            weak_unreachable: Gc::downgrade(unreachable),
        }
    });

    arena.mutate_root(|_, roots| {
        assert_eq!(roots.live.borrow().payload, 1);
        assert_eq!(roots.live.borrow().next.unwrap().borrow().payload, 2);
    });
    arena.finish_cycle();
    arena.mutate_root(|_, roots| assert!(roots.weak_unreachable.is_dropped()));

    // DynamicRootSet is the candidate mechanism for a pending continuation root.  It is
    // !Send/!Sync by design, matching ADR 0028's one-local-task realm ownership.
    let mut dynamic = Arena::<Rootable![DynamicRootSet<'_>]>::new(|mc| DynamicRootSet::new(mc));
    let handle: gc_arena::DynamicRoot<HeldRoot> = dynamic.mutate_root(|mc, roots| {
        let held = Gc::new(
            mc,
            RefLock::new(Node {
                next: None,
                payload: 7,
            }),
        );
        roots.stash(mc, Gc::new(mc, held))
    });
    dynamic.mutate_root(|mc, roots| {
        let held: Gc<'_, NodePtr<'_>> = roots.fetch(&handle);
        assert_eq!(held.borrow().payload, 7);
        let _ = mc;
    });
    drop(handle);
    dynamic.finish_cycle();
}
