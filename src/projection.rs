/// Given a type, projects a reference into that type as another type.
pub trait ProjectR<A: ?Sized, B: ?Sized>: Send + Sync {
    fn project<'a>(&self, a: &'a A) -> &'a B;
}

impl<A: ?Sized, B: ?Sized, T> ProjectR<A, B> for T
where
    Self: Send + Sync,
    T: Fn(&A) -> &B,
{
    fn project<'a>(&self, a: &'a A) -> &'a B {
        self(a)
    }
}

/// Given a type, projects a mutable reference into that type as another type.
pub trait ProjectW<A: ?Sized, B: ?Sized>: Send + Sync {
    fn project_mut<'a>(&self, a: &'a mut A) -> &'a mut B;
}

impl<A: ?Sized, B: ?Sized, T> ProjectW<A, B> for T
where
    Self: Send + Sync,
    T: Fn(&mut A) -> &mut B,
{
    fn project_mut<'a>(&self, a: &'a mut A) -> &'a mut B {
        self(a)
    }
}

pub enum RawOrProjection<L, P> {
    Raw(L),
    Projection(P),
}

impl<L: Clone, P: Clone> Clone for RawOrProjection<L, P> {
    fn clone(&self) -> Self {
        use RawOrProjection::*;
        match self {
            Raw(x) => Raw(x.clone()),
            Projection(x) => Projection(x.clone()),
        }
    }
}

/// Stores a read/write projection.
pub struct ProjectorRW<A: ?Sized, B: ?Sized> {
    pub ro: Box<dyn ProjectR<A, B>>,
    pub rw: Box<dyn ProjectW<A, B>>,
}

impl<A: ?Sized, B: ?Sized> ProjectorRW<A, B> {
    pub fn new(ro: impl ProjectR<A, B> + 'static, rw: impl ProjectW<A, B> + 'static) -> Self {
        Self {
            ro: Box::new(ro),
            rw: Box::new(rw),
        }
    }
}

/// Stores a read projection.
pub struct Projector<A: ?Sized, B: ?Sized> {
    pub ro: Box<dyn ProjectR<A, B>>,
}

impl<A: ?Sized, B: ?Sized> Projector<A, B> {
    pub fn new(ro: impl ProjectR<A, B> + 'static) -> Self {
        Self { ro: Box::new(ro) }
    }
}

/// Extract the [`Projector`] from a [`ProjectorRW`].
impl<A: ?Sized, B: ?Sized> From<ProjectorRW<A, B>> for Projector<A, B> {
    fn from(value: ProjectorRW<A, B>) -> Self {
        Self { ro: value.ro }
    }
}

#[macro_export]
macro_rules! project {
    ($x:ident : $type:ty, $expr:expr) => {{
        // We just need something with a type
        let $x: [$type; 0] = [];
        fn make_projection<A, B>(
            _: &[A; 0],
            a: impl (Fn(&A) -> &B) + Send + Sync + 'static,
            b: impl (Fn(&mut A) -> &mut B) + Send + Sync + 'static,
        ) -> $crate::ProjectorRW<A, B> {
            $crate::ProjectorRW::new(a, b)
        }
        make_projection(&$x, |$x: _| &$expr, |$x: _| &mut $expr)
    }};
}

#[cfg(test)]
mod test {
    #[test]
    fn test_projection() {
        let x = (1, 2);
        let projection1 = project!(x: (i32, i32), x.0);
        let projection2 = project!(x: (i32, i32), x.1);
        assert_eq!(1, *projection1.ro.project(&x));
        assert_eq!(2, *projection2.ro.project(&x));
    }
}
