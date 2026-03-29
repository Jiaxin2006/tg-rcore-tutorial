use core::cell::{RefCell, RefMut, UnsafeCell};
use core::ops::{Deref, DerefMut};
use riscv::register::sstatus;
use spin::Lazy;

/// 单处理器环境下的原始内部可变性容器。
pub struct UPSafeCellRaw<T> {
    inner: UnsafeCell<T>,
}

unsafe impl<T> Sync for UPSafeCellRaw<T> {}

impl<T> UPSafeCellRaw<T> {
    /// 仅限单处理器内核使用。
    pub unsafe fn new(value: T) -> Self {
        Self {
            inner: UnsafeCell::new(value),
        }
    }

    pub fn get_mut(&self) -> &mut T {
        unsafe { &mut (*self.inner.get()) }
    }
}

/// 中断屏蔽状态。
pub struct IntrMaskingInfo {
    nested_level: usize,
    sie_before_masking: bool,
}

impl IntrMaskingInfo {
    pub const fn new() -> Self {
        Self {
            nested_level: 0,
            sie_before_masking: false,
        }
    }

    pub fn enter(&mut self) {
        let sie = sstatus::read().sie();
        unsafe {
            sstatus::clear_sie();
        }
        if self.nested_level == 0 {
            self.sie_before_masking = sie;
        }
        self.nested_level += 1;
    }

    pub fn exit(&mut self) {
        self.nested_level -= 1;
        if self.nested_level == 0 && self.sie_before_masking {
            unsafe {
                sstatus::set_sie();
            }
        }
    }
}

/// 全局中断屏蔽状态。
pub static INTR_MASKING_INFO: Lazy<UPSafeCellRaw<IntrMaskingInfo>> = Lazy::new(|| unsafe {
    UPSafeCellRaw::new(IntrMaskingInfo::new())
});

/// 带关中断保护的单核内部可变性容器。
pub struct UPIntrFreeCell<T> {
    inner: RefCell<T>,
}

unsafe impl<T> Sync for UPIntrFreeCell<T> {}

/// 在持有关中断借用期间的可变引用包装。
pub struct UPIntrRefMut<'a, T>(Option<RefMut<'a, T>>);

impl<T> UPIntrFreeCell<T> {
    /// 仅限单处理器内核使用。
    pub unsafe fn new(value: T) -> Self {
        Self {
            inner: RefCell::new(value),
        }
    }

    pub fn exclusive_access(&self) -> UPIntrRefMut<'_, T> {
        INTR_MASKING_INFO.get_mut().enter();
        UPIntrRefMut(Some(self.inner.borrow_mut()))
    }

    pub fn exclusive_session<F, V>(&self, f: F) -> V
    where
        F: FnOnce(&mut T) -> V,
    {
        let mut inner = self.exclusive_access();
        f(inner.deref_mut())
    }
}

impl<'a, T> Drop for UPIntrRefMut<'a, T> {
    fn drop(&mut self) {
        self.0 = None;
        INTR_MASKING_INFO.get_mut().exit();
    }
}

impl<'a, T> Deref for UPIntrRefMut<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().unwrap().deref()
    }
}

impl<'a, T> DerefMut for UPIntrRefMut<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().unwrap().deref_mut()
    }
}
