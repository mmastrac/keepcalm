use crate::projection::{Projector, RawOrProjection};
use serde::Serialize;
use std::sync::Arc;

#[repr(transparent)]
pub struct Shared<T: ?Sized> {
    inner: RawOrProjection<Arc<T>, Arc<dyn SharedProjection<T>>>,
}

// UNSAFETY: The construction and projection of Shared requires Send + Sync, so we can guarantee that
// all instances of SharedRWImpl are Send + Sync.
unsafe impl<T: ?Sized> Send for Shared<T> {}
unsafe impl<T: ?Sized> Sync for Shared<T> {}

impl<'a, T: Serialize> Serialize for Shared<T>
where
    &'a T: Serialize + 'static,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        (**self).serialize(serializer)
    }
}

impl<T: ?Sized> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

trait SharedProjection<T: ?Sized>: Send + Sync {
    fn read(&self) -> &T;
}

impl<T: ?Sized> From<Box<T>> for Shared<T>
where
    Box<T>: Send + Sync,
{
    fn from(value: Box<T>) -> Self {
        Self {
            inner: RawOrProjection::Raw(Arc::from(value)),
        }
    }
}

impl<T: ?Sized> Shared<T> {
    pub fn from_box(t: Box<T>) -> Self
    where
        Box<T>: Send + Sync,
    {
        Self {
            inner: RawOrProjection::Raw(Arc::from(t)),
        }
    }
}

impl<T: Send + Sync + 'static> Shared<T> {
    pub fn new(t: T) -> Self {
        Self {
            inner: RawOrProjection::Raw(Arc::new(t)),
        }
    }
}

impl<T: ?Sized, P: ?Sized> SharedProjection<P> for (Shared<T>, Arc<Projector<T, P>>) {
    fn read(&self) -> &P {
        (self.1.ro).project(&*self.0)
    }
}

impl<T: ?Sized> std::ops::Deref for Shared<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        use RawOrProjection::*;
        match &self.inner {
            Raw(x) => x,
            Projection(x) => x.read(),
        }
    }
}

impl<T: ?Sized> Shared<T> {
    pub fn project<P: ?Sized + 'static, I: Into<Projector<T, P>>>(&self, projector: I) -> Shared<P>
    where
        T: 'static,
    {
        let projector: Projector<T, P> = projector.into();
        let projectable = Arc::new((self.clone(), Arc::new(projector)));
        Shared {
            inner: RawOrProjection::Projection(projectable),
        }
    }

    pub fn project_fn<P: ?Sized + 'static, RO: (Fn(&T) -> &P) + Send + Sync + 'static>(
        &self,
        ro: RO,
    ) -> Shared<P>
    where
        T: 'static,
    {
        let projectable = Arc::new((self.clone(), Arc::new(Projector::new(ro))));
        Shared {
            inner: RawOrProjection::Projection(projectable),
        }
    }
}

#[cfg(test)]
mod test {
    use crate::project_cast;

    use super::*;

    #[allow(unused)]
    fn ensure_send<T: Send>() {}
    #[allow(unused)]
    fn ensure_sync<T: Sync>() {}

    #[allow(unused)]
    fn test_types() {
        ensure_send::<Shared<usize>>();
        ensure_sync::<Shared<usize>>();
        ensure_send::<Shared<dyn AsRef<str>>>();
        ensure_sync::<Shared<dyn AsRef<str>>>();
    }

    #[test]
    pub fn test_shared() {
        let shared = Shared::new(1);
        assert_eq!(*shared, 1);
    }

    #[test]
    pub fn test_shared_projection() {
        let shared = Shared::new((1, 2));
        let shared_proj = shared.project_fn(|x| &x.0);
        assert_eq!(*shared_proj, 1);
        let shared_proj = shared.project_fn(|x| &x.1);
        assert_eq!(*shared_proj, 2);
    }

    #[test]
    pub fn test_unsized() {
        let shared: Shared<dyn AsRef<str>> =
            Shared::new("123".to_owned()).project(project_cast!(x: String => dyn AsRef<str>));
        assert_eq!(shared.as_ref(), "123");

        let shared: Shared<[i32]> = Shared::from_box(Box::new([1, 2, 3]));
        assert_eq!(shared[0], 1);
    }
}
