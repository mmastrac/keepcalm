pub enum RawOrProjection<L, P> {
    Lock(L),
    Projection(P),
}

impl<L: Clone, P: Clone> Clone for RawOrProjection<L, P> {
    fn clone(&self) -> Self {
        use RawOrProjection::*;
        match self {
            Lock(x) => Lock(x.clone()),
            Projection(x) => Projection(x.clone()),
        }
    }
}

/// Stores a read/write projection as two boxes.
pub struct ProjectorRW<A, B> {
    pub ro: Box<(dyn Fn(&A) -> &B + Send + Sync)>,
    pub rw: Box<(dyn Fn(&mut A) -> &mut B + Send + Sync)>,
}

impl<A, B> ProjectorRW<A, B> {
    pub fn new<
        RO: (Fn(&A) -> &B) + Send + Sync + 'static,
        RW: (Fn(&mut A) -> &mut B) + Send + Sync + 'static,
    >(
        ro: RO,
        rw: RW,
    ) -> Self {
        Self {
            ro: Box::new(ro),
            rw: Box::new(rw),
        }
    }
}

/// Stores a read/write projection as two boxes.
pub struct Projector<A, B> {
    pub ro: Box<(dyn Fn(&A) -> &B + Send + Sync)>,
}

impl<A, B> Projector<A, B> {
    pub fn new<RO: (Fn(&A) -> &B) + Send + Sync + 'static>(ro: RO) -> Self {
        Self { ro: Box::new(ro) }
    }
}
