use crate::projection::{Projector, RawOrProjection};
use serde::Serialize;
use std::sync::Arc;

#[repr(transparent)]
pub struct Shared<T> {
    inner: RawOrProjection<Arc<T>, Arc<Box<dyn SharedProjection<T> + Send + Sync>>>,
}

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

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

trait SharedProjection<T> {
    fn read<'a>(&'a self) -> &'a T;
}

impl<T: Send + Sync> Shared<T> {
    pub fn new(t: T) -> Self {
        Self {
            inner: RawOrProjection::Raw(Arc::new(t)),
        }
    }
}

impl<T: Send + Sync, P: Send + Sync> SharedProjection<P> for (Shared<T>, Arc<Projector<T, P>>) {
    fn read<'a>(&'a self) -> &'a P {
        (self.1.ro).project(&*self.0)
    }
}

impl<T> std::ops::Deref for Shared<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        use RawOrProjection::*;
        match &self.inner {
            Raw(x) => x,
            Projection(x) => x.read(),
        }
    }
}

impl<T: Send + Sync> Shared<T> {
    pub fn project_fn<P: Send + Sync + 'static, RO: (Fn(&T) -> &P) + Send + Sync + 'static>(
        &self,
        ro: RO,
    ) -> Shared<P>
    where
        T: 'static,
    {
        let projectable = (self.clone(), Arc::new(Projector::new(ro)));
        let projectable: Box<dyn SharedProjection<P> + Send + Sync> = Box::new(projectable);
        Shared {
            inner: RawOrProjection::Projection(Arc::new(projectable)),
        }
    }
}

#[cfg(test)]
mod test {
    use super::Shared;

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
}
