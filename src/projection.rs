/// Given a type, projects a reference into that type as another type.
pub trait ProjectR<A: ?Sized, B: ?Sized>: Send + Sync {
    fn project<'a>(&self, a: &'a A) -> &'a B;
}

impl<A: ?Sized, B: ?Sized, T> ProjectR<A, B> for T
where
    Self: Send + Sync,
    T: for<'a> Fn(&'a A) -> &'a B,
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
    T: for<'a> Fn(&'a mut A) -> &'a mut B,
{
    fn project_mut<'a>(&self, a: &'a mut A) -> &'a mut B {
        self(a)
    }
}

/// Projection as a method, generic over both [`crate::Shared`] and
/// [`crate::SharedMut`] containers.
///
/// This trait primarily exists to power the receiver form of [`project!`]:
/// `project!(container => field.path)`. Read-only containers ignore the
/// mutable projection function.
pub trait Project<T: ?Sized + 'static> {
    /// The projected container type: [`crate::Shared<P>`] for [`crate::Shared<T>`],
    /// [`crate::SharedMut<P>`] for [`crate::SharedMut<T>`].
    type Projected<P: ?Sized + 'static>;

    /// Project this container using a pair of projection functions.
    fn project_path<P: ?Sized + 'static>(
        &self,
        ro: impl for<'a> Fn(&'a T) -> &'a P + Send + Sync + 'static,
        rw: impl for<'a> Fn(&'a mut T) -> &'a mut P + Send + Sync + 'static,
    ) -> Self::Projected<P>;
}

/// Stores a read/write projection.
pub struct ProjectorRW<A: ?Sized, B: ?Sized> {
    pub(crate) ro: Box<dyn ProjectR<A, B> + Send + Sync>,
    pub(crate) rw: Box<dyn ProjectW<A, B> + Send + Sync>,
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
/// The preferred form takes the shared container and a field path, and returns
/// the projected container directly — no type annotations required, and the
/// path is written once:
///
/// ```rust
/// # use keepcalm::*;
/// #[derive(Default)]
/// struct Foo {
///     tuple: (String, usize),
/// }
///
/// let shared = SharedMut::new(Foo::default());
/// let shared_string = project!(shared => tuple.0);
/// *shared_string.write() += "hello, world";
/// assert_eq!(shared.read().tuple.0, "hello, world");
/// ```
///
/// This form works on both [`crate::SharedMut`] (yielding a `SharedMut`) and
/// [`crate::Shared`] (yielding a `Shared`), and the receiver may be any
/// expression. Tuple-index chains like `counters.1.1` are supported.
///
/// The original form builds a standalone [`ProjectorRW`], given a reference to
/// a type `A` projected into a reference of type `B`, where the reference for
/// `B` comes from somewhere within `A` (for example: an arbitrarily nested
/// field, part of a tuple, or part of a slice):
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
    ($shared:expr => $($path:tt)+) => {
        $crate::Project::project_path(&($shared), |x| &x.$($path)+, |x| &mut x.$($path)+)
    };
}

/// Projects a type as another type.
///
/// This performs a cast from one type to another, and is useful for creating generic shared objects based on traits rather than
/// concrete types, and without having to make methods generic.
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

/// A `const`-constructible table of lock functions that projects a global
/// container as a [`crate::SharedMut`].
///
/// Created by [`project_global!`]: the table is placed behind a `&'static`
/// reference (via an inline `const` block, which const-evaluation turns into
/// an anonymous static), so installing the projection performs no allocation
/// and can happen in a `static` initializer.
pub struct StaticProjection<T: ?Sized + 'static> {
    lock_read: fn() -> crate::SharedReadLock<'static, T>,
    try_lock_read: fn() -> Option<crate::SharedReadLock<'static, T>>,
    lock_write: fn() -> crate::SharedWriteLock<'static, T>,
    try_lock_write: fn() -> Option<crate::SharedWriteLock<'static, T>>,
}

impl<T: ?Sized + 'static> StaticProjection<T> {
    pub const fn new(
        lock_read: fn() -> crate::SharedReadLock<'static, T>,
        try_lock_read: fn() -> Option<crate::SharedReadLock<'static, T>>,
        lock_write: fn() -> crate::SharedWriteLock<'static, T>,
        try_lock_write: fn() -> Option<crate::SharedWriteLock<'static, T>>,
    ) -> Self {
        Self {
            lock_read,
            try_lock_read,
            lock_write,
            try_lock_write,
        }
    }
}

impl<T: ?Sized + 'static> crate::implementation::SharedMutProjection<T> for StaticProjection<T> {
    fn lock_read(&self) -> crate::SharedReadLock<'_, T> {
        (self.lock_read)()
    }
    fn try_lock_read(&self) -> Option<crate::SharedReadLock<'_, T>> {
        (self.try_lock_read)()
    }
    fn lock_write(&self) -> crate::SharedWriteLock<'_, T> {
        (self.lock_write)()
    }
    fn try_lock_write(&self) -> Option<crate::SharedWriteLock<'_, T>> {
        (self.try_lock_write)()
    }
}

/// The read-only counterpart of [`StaticProjection`], projecting a global
/// container as a [`crate::Shared`].
pub struct StaticProjectionRO<T: ?Sized + 'static> {
    read: fn() -> crate::SharedReadLock<'static, T>,
    try_lock_read: fn() -> Option<crate::SharedReadLock<'static, T>>,
}

impl<T: ?Sized + 'static> StaticProjectionRO<T> {
    pub const fn new(
        read: fn() -> crate::SharedReadLock<'static, T>,
        try_lock_read: fn() -> Option<crate::SharedReadLock<'static, T>>,
    ) -> Self {
        Self {
            read,
            try_lock_read,
        }
    }
}

impl<T: ?Sized + 'static> crate::implementation::SharedProjection<T> for StaticProjectionRO<T> {
    fn read(&self) -> crate::SharedReadLock<'_, T> {
        (self.read)()
    }
    fn try_lock_read(&self) -> Option<crate::SharedReadLock<'_, T>> {
        (self.try_lock_read)()
    }
}

/// Project a field of a global container ([`crate::SharedGlobal`] or
/// [`crate::SharedGlobalMut`]) as a [`crate::Shared`] or [`crate::SharedMut`],
/// in a `const` context.
///
/// `project_global!(GLOBAL => path)` returns a read-only [`crate::Shared`];
/// `project_global!(mut GLOBAL => path)` returns a [`crate::SharedMut`] (and
/// requires the global to be a [`crate::SharedGlobalMut`]). Because the
/// projection is built entirely from function pointers, the result can
/// initialize another `static`:
///
/// ```rust
/// # use keepcalm::*;
/// struct App {
///     name: String,
///     port: u16,
/// }
/// static APP: SharedGlobalMut<App> = SharedGlobalMut::new_lazy(|| App {
///     name: "server".to_string(),
///     port: 80,
/// });
/// static NAME: SharedMut<String> = project_global!(mut APP => name);
/// static PORT: Shared<u16> = project_global!(APP => port);
///
/// *NAME.write() += "-1";
/// assert_eq!(APP.read().name, "server-1");
/// assert_eq!(*PORT.read(), 80);
/// ```
///
/// The global must be referred to by its path (e.g. `APP` or `config::APP`):
/// the generated lock functions are non-capturing closures, so a local
/// variable cannot be used here.
#[macro_export]
macro_rules! project_global {
    (mut $global:expr => $($path:tt)+) => {
        $crate::SharedMut::from_static_projection(const {
            &$crate::StaticProjection::new(
                || $global.read().map(|x| &x.$($path)+),
                || $global.try_read().map(|g| g.map(|x| &x.$($path)+)),
                || $global.write().map(|x| &x.$($path)+, |x| &mut x.$($path)+),
                || $global
                    .try_write()
                    .map(|g| g.map(|x| &x.$($path)+, |x| &mut x.$($path)+)),
            )
        })
    };
    ($global:expr => $($path:tt)+) => {
        $crate::Shared::from_static_projection(const {
            &$crate::StaticProjectionRO::new(
                || $global.read().map(|x| &x.$($path)+),
                || $global.try_read().map(|g| g.map(|x| &x.$($path)+)),
            )
        })
    };
}

/// A trait that allows a type to be cast to another, generically unsized type.
pub trait Castable<T: 'static>
where
    T: ?Sized,
{
    fn cast<'a>(&'a self) -> &'a T
    where
        Self: 'a;
    fn cast_mut<'a>(&'a mut self) -> &'a mut T
    where
        Self: 'a;
}

macro_rules! impl_castable_trait {
    ($trait:path) => {
        impl<T_> Castable<dyn $trait + 'static> for T_
        where
            T_: $trait + 'static,
        {
            fn cast<'a>(&'a self) -> &'a (dyn $trait + 'static)
            where
                Self: 'a,
            {
                self
            }
            fn cast_mut<'a>(&'a mut self) -> &'a mut (dyn $trait + 'static)
            where
                Self: 'a,
            {
                self
            }
        }
    };
}

// Implement for common std traits
impl_castable_trait!(std::any::Any);
impl_castable_trait!(std::fmt::Debug);
impl_castable_trait!(std::fmt::Display);

macro_rules! impl_castable_ref_trait {
    ($type:ident, $trait:path) => {
        impl<T_, $type: 'static> Castable<dyn $trait + 'static> for T_
        where
            T_: $trait + 'static,
            $type: ?Sized,
        {
            fn cast<'a>(&'a self) -> &'a (dyn $trait + 'static)
            where
                Self: 'a,
            {
                self
            }
            fn cast_mut<'a>(&'a mut self) -> &'a mut (dyn $trait + 'static)
            where
                Self: 'a,
            {
                self
            }
        }
    };
}

// Implement for common std traits involving references
impl_castable_ref_trait!(T, std::convert::AsRef<T>);
impl_castable_ref_trait!(T, std::convert::AsMut<T>);
impl_castable_ref_trait!(T, std::ops::Deref<Target = T>);
impl_castable_ref_trait!(T, std::ops::DerefMut<Target = T>);
impl_castable_ref_trait!(T, std::borrow::Borrow<T>);
impl_castable_ref_trait!(T, std::borrow::BorrowMut<T>);

impl<U: 'static, const N: usize> Castable<[U]> for [U; N] {
    fn cast<'a>(&'a self) -> &'a [U]
    where
        Self: 'a,
    {
        self
    }
    fn cast_mut<'a>(&'a mut self) -> &'a mut [U]
    where
        Self: 'a,
    {
        self
    }
}

impl<T: ?Sized, U: ?Sized + 'static> ProjectR<T, U> for ()
where
    T: Castable<U>,
{
    fn project<'a>(&self, t: &'a T) -> &'a U {
        t.cast()
    }
}

impl<T: ?Sized, U: ?Sized + 'static> ProjectW<T, U> for ()
where
    T: Castable<U>,
{
    fn project_mut<'a>(&self, t: &'a mut T) -> &'a mut U {
        t.cast_mut()
    }
}

#[cfg(test)]
mod test {
    #[test]
    fn test_project_receiver_form() {
        #[derive(Default)]
        struct Config {
            name: String,
        }
        #[derive(Default)]
        struct App {
            config: Config,
            counters: (u64, (u64, u64)),
        }

        let shared = crate::SharedMut::new(App::default());

        // Nested field path, no type annotations.
        let name = project!(shared => config.name);
        *name.write() += "hello";

        // Tuple-index chains, including the `1.1` float-literal token form.
        let deep = project!(shared => counters.1.1);
        *deep.write() += 42;

        // Read-only containers project to Shared.
        let ro: crate::Shared<App> = shared.shared_copy();
        let ro_name: crate::Shared<String> = project!(ro => config.name);
        assert_eq!(*ro_name.read(), "hello");

        // The receiver may be any expression.
        struct Wrapper {
            state: crate::SharedMut<App>,
        }
        let wrapper = Wrapper {
            state: shared.clone(),
        };
        let name2 = project!(wrapper.state => config.name);
        assert_eq!(*name2.read(), "hello");

        assert_eq!(shared.read().counters.1 .1, 42);
    }

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
        let projection = project_cast!(x: String => dyn AsRefMut<str>);
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
