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

/// Stores a read/write projection.
pub struct ProjectorRW<A: ?Sized, B: ?Sized> {
    ro: Box<dyn ProjectR<A, B> + Send + Sync>,
    rw: Box<dyn ProjectW<A, B> + Send + Sync>,
}

impl<A: ?Sized, B: ?Sized> ProjectorRW<A, B> {
    pub fn new(ro: impl ProjectR<A, B> + 'static, rw: impl ProjectW<A, B> + 'static) -> Self {
        Self {
            ro: Box::new(ro),
            rw: Box::new(rw),
        }
    }

    pub fn project<'a>(&self, a: &'a A) -> &'a B {
        self.ro.project(a)
    }

    pub fn project_mut<'a>(&self, a: &'a mut A) -> &'a mut B {
        self.rw.project_mut(a)
    }
}

/// Stores a read projection.
pub struct Projector<A: ?Sized, B: ?Sized> {
    ro: Box<dyn ProjectR<A, B> + Send + Sync>,
}

impl<A: ?Sized, B: ?Sized> Projector<A, B> {
    pub fn new(ro: impl ProjectR<A, B> + 'static) -> Self {
        Self { ro: Box::new(ro) }
    }

    pub fn project<'a>(&self, a: &'a A) -> &'a B {
        self.ro.project(a)
    }
}

/// Extract the [`Projector`] from a [`ProjectorRW`].
impl<A: ?Sized, B: ?Sized> From<ProjectorRW<A, B>> for Projector<A, B> {
    fn from(value: ProjectorRW<A, B>) -> Self {
        Self { ro: value.ro }
    }
}

/// Project part of a type as another type.
///
/// ```rust
/// # use keepcalm::*;
/// // Creates two projections for each field of a tuple:
/// let projection0 = project!(x: (i32, i32), x.0);
/// let projection1 = project!(x: (i32, i32), x.1);
///
/// assert_eq!(1, *projection0.project(&(1, 2)));
/// assert_eq!(2, *projection1.project(&(1, 2)));
/// ```
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

/// Projects a type as another type.
///
/// ```rust
/// # use keepcalm::*;
/// let projection = project_cast!(x: [i32; 3] => dyn std::ops::IndexMut<usize, Output = i32>);
///
/// let mut x = [1, 2, 3];
/// projection.project_mut(&mut x)[0] += 10;
/// assert_eq!(projection.project(&x)[0], 11);
/// ```
#[macro_export]
macro_rules! project_cast {
    ($x:ident : $type:ty => $type2:ty) => {{
        // We just need something with a type
        fn make_projection<A: ?Sized, B: ?Sized>(
            a: impl (Fn(&A) -> &B) + Send + Sync + 'static,
            b: impl (Fn(&mut A) -> &mut B) + Send + Sync + 'static,
        ) -> $crate::ProjectorRW<A, B> {
            $crate::ProjectorRW::new(a, b)
        }
        make_projection(
            |$x: &$type| $x as &$type2,
            |$x: &mut $type| $x as &mut $type2,
        )
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

    #[test]
    fn test_projection_cast() {
        trait AsRefMut<T: ?Sized>: AsRef<T> + AsMut<T> {}
        impl AsRefMut<str> for String {}

        let mut x: String = "123".into();
        let projection = project_cast!(x: String => (dyn AsRefMut<str>));
        assert_eq!(projection.project(&x).as_ref(), "123");
        assert_eq!(projection.project_mut(&mut x).as_mut(), "123");
    }

    #[test]
    fn test_projection_cast_array() {
        let projection = project_cast!(x: [i32; 3] => dyn std::ops::IndexMut<usize, Output = i32>);
        let mut x = [1, 2, 3];
        projection.project_mut(&mut x)[0] = 11;
    }
}
