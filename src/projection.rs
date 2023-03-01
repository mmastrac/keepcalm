pub trait ProjectR<A, B>: Send + Sync {
    fn project<'a>(&self, a: &'a A) -> &'a B;
}

impl<A, B, T> ProjectR<A, B> for T
where
    Self: Send + Sync,
    T: Fn(&A) -> &B,
{
    fn project<'a>(&self, a: &'a A) -> &'a B {
        self(a)
    }
}

pub trait ProjectW<A, B>: Send + Sync {
    fn project_mut<'a>(&self, a: &'a mut A) -> &'a mut B;
}

impl<A, B, T> ProjectW<A, B> for T
where
    Self: Send + Sync,
    T: Fn(&mut A) -> &mut B,
{
    fn project_mut<'a>(&self, a: &'a mut A) -> &'a mut B {
        self(a)
    }
}

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
    pub ro: Box<dyn ProjectR<A, B>>,
    pub rw: Box<dyn ProjectW<A, B>>,
}

impl<A, B> ProjectorRW<A, B> {
    pub fn new(ro: impl ProjectR<A, B> + 'static, rw: impl ProjectW<A, B> + 'static) -> Self {
        Self {
            ro: Box::new(ro),
            rw: Box::new(rw),
        }
    }
}

/// Stores a read/write projection as two boxes.
pub struct Projector<A, B> {
    pub ro: Box<dyn ProjectR<A, B>>,
}

impl<A, B> Projector<A, B> {
    pub fn new(ro: impl ProjectR<A, B> + 'static) -> Self {
        Self { ro: Box::new(ro) }
    }
}
