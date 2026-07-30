//! Safe Rust boundary around the optional CUDA NCM4 scoring kernel.
//!
//! This crate is deliberately separate from `pouw-core`: CUDA is a search
//! accelerator and never participates in consensus decoding or verification.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::fmt;
use std::mem::{self, MaybeUninit};
use std::ptr;

use libloading::Library;

const PTX: &str = include_str!("../kernels/ncm4_score.ptx");
const KERNEL_NAME: &[u8] = b"nicechunk_score_ncm4\0";
const CUDA_SUCCESS: i32 = 0;

pub const PACKED_OP_PARAMETER_COUNT: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum PackedOpKind {
    Box = 0,
    RepeatBox = 1,
    Gable = 2,
    Tree = 3,
    Fence = 4,
    Run = 5,
    Wall = 6,
    Extrude = 7,
    Translate = 8,
    RotateY = 9,
    Mirror = 10,
    RepeatRegion = 11,
    ClearBox = 12,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct PackedOp {
    pub kind: u32,
    pub parameters: [i32; PACKED_OP_PARAMETER_COUNT],
}

impl PackedOp {
    pub const fn new(kind: PackedOpKind) -> Self {
        Self {
            kind: kind as u32,
            parameters: [0; PACKED_OP_PARAMETER_COUNT],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct PackedScore {
    pub valid: u32,
    pub mismatches: u32,
    pub set_patches: u32,
    pub clear_patches: u32,
    pub paint_patches: u32,
    pub patch_runs: u32,
    pub writes: u32,
    pub reserved: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CudaDeviceInfo {
    pub ordinal: u32,
    pub name: String,
    pub compute_major: i32,
    pub compute_minor: i32,
    pub total_memory_bytes: usize,
    pub driver_version: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CudaError {
    operation: &'static str,
    detail: String,
}

impl CudaError {
    fn loading(detail: impl Into<String>) -> Self {
        Self {
            operation: "load-driver",
            detail: detail.into(),
        }
    }

    fn invalid(detail: impl Into<String>) -> Self {
        Self {
            operation: "invalid-batch",
            detail: detail.into(),
        }
    }

    fn driver(operation: &'static str, code: i32, api: Option<&CudaApi>) -> Self {
        let description = api
            .and_then(|api| api.error_description(code))
            .unwrap_or_else(|| format!("CUDA driver error {code}"));
        Self {
            operation,
            detail: description,
        }
    }

    pub fn is_unavailable(&self) -> bool {
        matches!(
            self.operation,
            "load-driver" | "cuInit" | "cuDeviceGetCount" | "cuDeviceGet"
        )
    }
}

impl fmt::Display for CudaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.operation, self.detail)
    }
}

impl std::error::Error for CudaError {}

type CuResult = c_int;
type CuDevice = c_int;
type CuContext = *mut c_void;
type CuModule = *mut c_void;
type CuFunction = *mut c_void;
type CuStream = *mut c_void;
type CuDevicePtr = u64;

type CuInit = unsafe extern "C" fn(c_uint) -> CuResult;
type CuDriverGetVersion = unsafe extern "C" fn(*mut c_int) -> CuResult;
type CuDeviceGetCount = unsafe extern "C" fn(*mut c_int) -> CuResult;
type CuDeviceGet = unsafe extern "C" fn(*mut CuDevice, c_int) -> CuResult;
type CuDeviceGetName = unsafe extern "C" fn(*mut c_char, c_int, CuDevice) -> CuResult;
type CuDeviceComputeCapability = unsafe extern "C" fn(*mut c_int, *mut c_int, CuDevice) -> CuResult;
type CuDeviceTotalMem = unsafe extern "C" fn(*mut usize, CuDevice) -> CuResult;
type CuCtxCreate = unsafe extern "C" fn(*mut CuContext, c_uint, CuDevice) -> CuResult;
type CuCtxDestroy = unsafe extern "C" fn(CuContext) -> CuResult;
type CuCtxSetCurrent = unsafe extern "C" fn(CuContext) -> CuResult;
type CuModuleLoadData = unsafe extern "C" fn(*mut CuModule, *const c_void) -> CuResult;
type CuModuleUnload = unsafe extern "C" fn(CuModule) -> CuResult;
type CuModuleGetFunction =
    unsafe extern "C" fn(*mut CuFunction, CuModule, *const c_char) -> CuResult;
type CuMemAlloc = unsafe extern "C" fn(*mut CuDevicePtr, usize) -> CuResult;
type CuMemFree = unsafe extern "C" fn(CuDevicePtr) -> CuResult;
type CuMemcpyHtoD = unsafe extern "C" fn(CuDevicePtr, *const c_void, usize) -> CuResult;
type CuMemcpyDtoH = unsafe extern "C" fn(*mut c_void, CuDevicePtr, usize) -> CuResult;
type CuLaunchKernel = unsafe extern "C" fn(
    CuFunction,
    c_uint,
    c_uint,
    c_uint,
    c_uint,
    c_uint,
    c_uint,
    c_uint,
    CuStream,
    *mut *mut c_void,
    *mut *mut c_void,
) -> CuResult;
type CuCtxSynchronize = unsafe extern "C" fn() -> CuResult;
type CuGetErrorString = unsafe extern "C" fn(CuResult, *mut *const c_char) -> CuResult;

struct CudaApi {
    _library: Library,
    init: CuInit,
    driver_get_version: CuDriverGetVersion,
    device_get_count: CuDeviceGetCount,
    device_get: CuDeviceGet,
    device_get_name: CuDeviceGetName,
    device_compute_capability: CuDeviceComputeCapability,
    device_total_mem: CuDeviceTotalMem,
    ctx_create: CuCtxCreate,
    ctx_destroy: CuCtxDestroy,
    ctx_set_current: CuCtxSetCurrent,
    module_load_data: CuModuleLoadData,
    module_unload: CuModuleUnload,
    module_get_function: CuModuleGetFunction,
    mem_alloc: CuMemAlloc,
    mem_free: CuMemFree,
    memcpy_htod: CuMemcpyHtoD,
    memcpy_dtoh: CuMemcpyDtoH,
    launch_kernel: CuLaunchKernel,
    ctx_synchronize: CuCtxSynchronize,
    get_error_string: CuGetErrorString,
}

impl CudaApi {
    fn load() -> Result<Self, CudaError> {
        #[cfg(target_os = "windows")]
        let library_name = "nvcuda.dll";
        #[cfg(not(target_os = "windows"))]
        let library_name = "libcuda.so.1";

        // SAFETY: the library handle is retained by CudaApi for at least as
        // long as every copied function pointer. Every symbol is loaded with
        // the signature from the CUDA Driver API.
        unsafe {
            let library = Library::new(library_name)
                .map_err(|error| CudaError::loading(error.to_string()))?;
            macro_rules! symbol {
                ($name:literal, $kind:ty) => {
                    *library
                        .get::<$kind>($name)
                        .map_err(|error| CudaError::loading(error.to_string()))?
                };
            }
            Ok(Self {
                init: symbol!(b"cuInit\0", CuInit),
                driver_get_version: symbol!(b"cuDriverGetVersion\0", CuDriverGetVersion),
                device_get_count: symbol!(b"cuDeviceGetCount\0", CuDeviceGetCount),
                device_get: symbol!(b"cuDeviceGet\0", CuDeviceGet),
                device_get_name: symbol!(b"cuDeviceGetName\0", CuDeviceGetName),
                device_compute_capability: symbol!(
                    b"cuDeviceComputeCapability\0",
                    CuDeviceComputeCapability
                ),
                device_total_mem: symbol!(b"cuDeviceTotalMem_v2\0", CuDeviceTotalMem),
                ctx_create: symbol!(b"cuCtxCreate_v2\0", CuCtxCreate),
                ctx_destroy: symbol!(b"cuCtxDestroy_v2\0", CuCtxDestroy),
                ctx_set_current: symbol!(b"cuCtxSetCurrent\0", CuCtxSetCurrent),
                module_load_data: symbol!(b"cuModuleLoadData\0", CuModuleLoadData),
                module_unload: symbol!(b"cuModuleUnload\0", CuModuleUnload),
                module_get_function: symbol!(b"cuModuleGetFunction\0", CuModuleGetFunction),
                mem_alloc: symbol!(b"cuMemAlloc_v2\0", CuMemAlloc),
                mem_free: symbol!(b"cuMemFree_v2\0", CuMemFree),
                memcpy_htod: symbol!(b"cuMemcpyHtoD_v2\0", CuMemcpyHtoD),
                memcpy_dtoh: symbol!(b"cuMemcpyDtoH_v2\0", CuMemcpyDtoH),
                launch_kernel: symbol!(b"cuLaunchKernel\0", CuLaunchKernel),
                ctx_synchronize: symbol!(b"cuCtxSynchronize\0", CuCtxSynchronize),
                get_error_string: symbol!(b"cuGetErrorString\0", CuGetErrorString),
                _library: library,
            })
        }
    }

    fn check(&self, operation: &'static str, code: CuResult) -> Result<(), CudaError> {
        if code == CUDA_SUCCESS {
            Ok(())
        } else {
            Err(CudaError::driver(operation, code, Some(self)))
        }
    }

    fn error_description(&self, code: CuResult) -> Option<String> {
        let mut pointer = ptr::null();
        // SAFETY: cuGetErrorString writes a static NUL-terminated string
        // pointer on success. It is copied before the driver can be unloaded.
        let result = unsafe { (self.get_error_string)(code, &mut pointer) };
        if result != CUDA_SUCCESS || pointer.is_null() {
            None
        } else {
            // SAFETY: CUDA guarantees pointer is a valid NUL-terminated string.
            Some(
                unsafe { CStr::from_ptr(pointer) }
                    .to_string_lossy()
                    .into_owned(),
            )
        }
    }
}

pub fn devices() -> Result<Vec<CudaDeviceInfo>, CudaError> {
    let api = CudaApi::load()?;
    // SAFETY: all pointers passed to the CUDA API reference initialized local
    // storage of the documented size.
    unsafe {
        api.check("cuInit", (api.init)(0))?;
        let mut driver_version = 0;
        api.check(
            "cuDriverGetVersion",
            (api.driver_get_version)(&mut driver_version),
        )?;
        let mut count = 0;
        api.check("cuDeviceGetCount", (api.device_get_count)(&mut count))?;
        let mut output = Vec::with_capacity(count.max(0) as usize);
        for ordinal in 0..count {
            let mut device = 0;
            api.check("cuDeviceGet", (api.device_get)(&mut device, ordinal))?;
            let mut name = [0_i8; 256];
            api.check(
                "cuDeviceGetName",
                (api.device_get_name)(name.as_mut_ptr(), name.len() as c_int, device),
            )?;
            let mut major = 0;
            let mut minor = 0;
            api.check(
                "cuDeviceComputeCapability",
                (api.device_compute_capability)(&mut major, &mut minor, device),
            )?;
            let mut memory = 0;
            api.check(
                "cuDeviceTotalMem",
                (api.device_total_mem)(&mut memory, device),
            )?;
            output.push(CudaDeviceInfo {
                ordinal: ordinal as u32,
                name: CStr::from_ptr(name.as_ptr()).to_string_lossy().into_owned(),
                compute_major: major,
                compute_minor: minor,
                total_memory_bytes: memory,
                driver_version,
            });
        }
        Ok(output)
    }
}

#[derive(Default)]
struct DeviceAllocation {
    pointer: CuDevicePtr,
    capacity: usize,
}

impl DeviceAllocation {
    fn ensure(&mut self, api: &CudaApi, bytes: usize) -> Result<(), CudaError> {
        let bytes = bytes.max(1);
        if self.capacity >= bytes {
            return Ok(());
        }
        if self.pointer != 0 {
            // SAFETY: pointer was returned by cuMemAlloc in this context and
            // has not previously been freed.
            unsafe { api.check("cuMemFree", (api.mem_free)(self.pointer))? };
            self.pointer = 0;
            self.capacity = 0;
        }
        let mut pointer = 0;
        // SAFETY: CUDA writes one device pointer into valid local storage.
        unsafe { api.check("cuMemAlloc", (api.mem_alloc)(&mut pointer, bytes))? };
        self.pointer = pointer;
        self.capacity = bytes;
        Ok(())
    }

    fn free(&mut self, api: &CudaApi) {
        if self.pointer != 0 {
            // SAFETY: best-effort cleanup of an allocation owned by this value.
            let _ = unsafe { (api.mem_free)(self.pointer) };
            self.pointer = 0;
            self.capacity = 0;
        }
    }
}

pub struct CudaScorer {
    api: CudaApi,
    context: CuContext,
    module: CuModule,
    function: CuFunction,
    device: CudaDeviceInfo,
    dimensions: [u16; 3],
    volume: u32,
    max_expanded_per_op: u32,
    max_writes: u32,
    target: DeviceAllocation,
    operations: DeviceAllocation,
    offsets: DeviceAllocation,
    masks: DeviceAllocation,
    scenes: DeviceAllocation,
    snapshots: DeviceAllocation,
    scores: DeviceAllocation,
}

impl CudaScorer {
    pub fn new(
        device_ordinal: u32,
        dimensions: [u16; 3],
        target: &[u16],
        max_expanded_per_op: u32,
        max_writes: u32,
    ) -> Result<Self, CudaError> {
        let volume = dimensions
            .iter()
            .try_fold(1_u32, |value, dimension| {
                value.checked_mul(u32::from(*dimension))
            })
            .ok_or_else(|| CudaError::invalid("building volume overflows u32"))?;
        if dimensions.contains(&0) || target.len() != volume as usize {
            return Err(CudaError::invalid(
                "target length does not match the non-zero building dimensions",
            ));
        }
        let available = devices()?;
        let device = available
            .into_iter()
            .find(|value| value.ordinal == device_ordinal)
            .ok_or_else(|| CudaError::invalid("CUDA device ordinal is unavailable"))?;
        if device.compute_major < 7 {
            return Err(CudaError::invalid(
                "CUDA evaluator requires compute capability 7.0 or newer",
            ));
        }

        let api = CudaApi::load()?;
        let mut cuda_device = 0;
        let mut context = ptr::null_mut();
        let mut module = ptr::null_mut();
        let mut function = ptr::null_mut();
        let ptx = CString::new(PTX)
            .map_err(|_| CudaError::invalid("embedded PTX contains an interior NUL"))?;
        // SAFETY: each CUDA output pointer references valid local storage and
        // the PTX/name strings remain alive for the duration of each call.
        unsafe {
            api.check("cuInit", (api.init)(0))?;
            api.check(
                "cuDeviceGet",
                (api.device_get)(&mut cuda_device, device_ordinal as c_int),
            )?;
            api.check(
                "cuCtxCreate",
                (api.ctx_create)(&mut context, 0, cuda_device),
            )?;
            if let Err(error) = api.check(
                "cuModuleLoadData",
                (api.module_load_data)(&mut module, ptx.as_ptr().cast()),
            ) {
                let _ = (api.ctx_destroy)(context);
                return Err(error);
            }
            if let Err(error) = api.check(
                "cuModuleGetFunction",
                (api.module_get_function)(&mut function, module, KERNEL_NAME.as_ptr().cast()),
            ) {
                let _ = (api.module_unload)(module);
                let _ = (api.ctx_destroy)(context);
                return Err(error);
            }
        }

        let mut scorer = Self {
            api,
            context,
            module,
            function,
            device,
            dimensions,
            volume,
            max_expanded_per_op,
            max_writes,
            target: DeviceAllocation::default(),
            operations: DeviceAllocation::default(),
            offsets: DeviceAllocation::default(),
            masks: DeviceAllocation::default(),
            scenes: DeviceAllocation::default(),
            snapshots: DeviceAllocation::default(),
            scores: DeviceAllocation::default(),
        };
        let target_bytes = mem::size_of_val(target);
        scorer.target.ensure(&scorer.api, target_bytes)?;
        scorer.copy_to_device(scorer.target.pointer, target, "target")?;
        Ok(scorer)
    }

    pub fn device(&self) -> &CudaDeviceInfo {
        &self.device
    }

    pub fn score(
        &mut self,
        operations: &[PackedOp],
        offsets: &[u32],
        masks: &[u8],
    ) -> Result<Vec<PackedScore>, CudaError> {
        if offsets.len() < 2
            || offsets[0] != 0
            || offsets.windows(2).any(|pair| pair[0] > pair[1])
            || offsets.last().copied() != Some(operations.len() as u32)
        {
            return Err(CudaError::invalid(
                "operation offsets must be sorted, start at zero, and cover all operations",
            ));
        }
        let candidate_count = u32::try_from(offsets.len() - 1)
            .map_err(|_| CudaError::invalid("candidate count exceeds u32"))?;
        if candidate_count == 0 {
            return Err(CudaError::invalid("CUDA batch cannot be empty"));
        }
        let scene_elements = usize::try_from(candidate_count)
            .ok()
            .and_then(|count| count.checked_mul(self.volume as usize))
            .ok_or_else(|| CudaError::invalid("CUDA scene batch size overflows usize"))?;
        let scene_bytes = scene_elements
            .checked_mul(mem::size_of::<u16>())
            .ok_or_else(|| CudaError::invalid("CUDA scene byte size overflows usize"))?;
        let combined_working_bytes = scene_bytes
            .checked_mul(2)
            .ok_or_else(|| CudaError::invalid("CUDA working set overflows usize"))?;
        if combined_working_bytes > self.device.total_memory_bytes.saturating_mul(3) / 4 {
            return Err(CudaError::invalid(
                "CUDA batch requires more than 75% of device memory",
            ));
        }

        // SAFETY: this context belongs to the scorer and remains alive.
        unsafe {
            self.api
                .check("cuCtxSetCurrent", (self.api.ctx_set_current)(self.context))?;
        }
        self.operations
            .ensure(&self.api, mem::size_of_val(operations))?;
        self.offsets.ensure(&self.api, mem::size_of_val(offsets))?;
        self.masks.ensure(&self.api, masks.len())?;
        self.scenes.ensure(&self.api, scene_bytes)?;
        self.snapshots.ensure(&self.api, scene_bytes)?;
        let score_bytes = (candidate_count as usize)
            .checked_mul(mem::size_of::<PackedScore>())
            .ok_or_else(|| CudaError::invalid("CUDA score size overflows usize"))?;
        self.scores.ensure(&self.api, score_bytes)?;

        self.copy_to_device(self.operations.pointer, operations, "operations")?;
        self.copy_to_device(self.offsets.pointer, offsets, "offsets")?;
        if !masks.is_empty() {
            self.copy_to_device(self.masks.pointer, masks, "masks")?;
        }

        let mut operations_pointer = self.operations.pointer;
        let mut offsets_pointer = self.offsets.pointer;
        let mut masks_pointer = self.masks.pointer;
        let mut mask_count = u32::try_from(masks.len())
            .map_err(|_| CudaError::invalid("CUDA mask bytes exceed u32"))?;
        let mut target_pointer = self.target.pointer;
        let mut scenes_pointer = self.scenes.pointer;
        let mut snapshots_pointer = self.snapshots.pointer;
        let mut volume = self.volume;
        let mut size_x = u32::from(self.dimensions[0]);
        let mut size_y = u32::from(self.dimensions[1]);
        let mut size_z = u32::from(self.dimensions[2]);
        let mut candidates = candidate_count;
        let mut maximum_expansion = self.max_expanded_per_op;
        let mut maximum_writes = self.max_writes;
        let mut scores_pointer = self.scores.pointer;
        let mut parameters = [
            (&mut operations_pointer as *mut CuDevicePtr).cast::<c_void>(),
            (&mut offsets_pointer as *mut CuDevicePtr).cast::<c_void>(),
            (&mut masks_pointer as *mut CuDevicePtr).cast::<c_void>(),
            (&mut mask_count as *mut u32).cast::<c_void>(),
            (&mut target_pointer as *mut CuDevicePtr).cast::<c_void>(),
            (&mut scenes_pointer as *mut CuDevicePtr).cast::<c_void>(),
            (&mut snapshots_pointer as *mut CuDevicePtr).cast::<c_void>(),
            (&mut volume as *mut u32).cast::<c_void>(),
            (&mut size_x as *mut u32).cast::<c_void>(),
            (&mut size_y as *mut u32).cast::<c_void>(),
            (&mut size_z as *mut u32).cast::<c_void>(),
            (&mut candidates as *mut u32).cast::<c_void>(),
            (&mut maximum_expansion as *mut u32).cast::<c_void>(),
            (&mut maximum_writes as *mut u32).cast::<c_void>(),
            (&mut scores_pointer as *mut CuDevicePtr).cast::<c_void>(),
        ];
        // SAFETY: kernel argument pointers reference live local scalars with
        // layouts matching the CUDA C signature. Device buffers are sized and
        // initialized above. The context is current on this thread.
        unsafe {
            self.api.check(
                "cuLaunchKernel",
                (self.api.launch_kernel)(
                    self.function,
                    candidate_count,
                    1,
                    1,
                    256,
                    1,
                    1,
                    0,
                    ptr::null_mut(),
                    parameters.as_mut_ptr(),
                    ptr::null_mut(),
                ),
            )?;
            self.api
                .check("cuCtxSynchronize", (self.api.ctx_synchronize)())?;
        }

        let mut output = vec![MaybeUninit::<PackedScore>::uninit(); candidate_count as usize];
        // SAFETY: output has exactly score_bytes writable bytes. CUDA has
        // synchronized, and the kernel initializes every PackedScore field.
        unsafe {
            self.api.check(
                "cuMemcpyDtoH(scores)",
                (self.api.memcpy_dtoh)(
                    output.as_mut_ptr().cast(),
                    self.scores.pointer,
                    score_bytes,
                ),
            )?;
            Ok(output
                .into_iter()
                .map(|score| score.assume_init())
                .collect())
        }
    }

    fn copy_to_device<T>(
        &self,
        destination: CuDevicePtr,
        values: &[T],
        label: &'static str,
    ) -> Result<(), CudaError> {
        let bytes = mem::size_of_val(values);
        if bytes == 0 {
            return Ok(());
        }
        // SAFETY: source contains bytes readable for the duration of the
        // synchronous copy and destination has been allocated to fit them.
        let code = unsafe { (self.api.memcpy_htod)(destination, values.as_ptr().cast(), bytes) };
        self.api.check(label, code).map_err(|error| CudaError {
            operation: "cuMemcpyHtoD",
            detail: error.to_string(),
        })
    }
}

impl Drop for CudaScorer {
    fn drop(&mut self) {
        // SAFETY: cleanup is best effort and all handles are owned by self.
        unsafe {
            let _ = (self.api.ctx_set_current)(self.context);
        }
        self.scores.free(&self.api);
        self.snapshots.free(&self.api);
        self.scenes.free(&self.api);
        self.masks.free(&self.api);
        self.offsets.free(&self.api);
        self.operations.free(&self.api);
        self.target.free(&self.api);
        // SAFETY: module and context were created by this scorer and are
        // destroyed after all allocations have been released.
        unsafe {
            if !self.module.is_null() {
                let _ = (self.api.module_unload)(self.module);
            }
            if !self.context.is_null() {
                let _ = (self.api.ctx_destroy)(self.context);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuda_host_layout_matches_kernel_abi() {
        assert_eq!(mem::size_of::<PackedOp>(), 84);
        assert_eq!(mem::align_of::<PackedOp>(), 4);
        assert_eq!(mem::size_of::<PackedScore>(), 32);
        assert_eq!(mem::align_of::<PackedScore>(), 4);
        assert!(PTX.contains(".visible .entry nicechunk_score_ncm4("));
        assert!(PTX.contains(".target sm_70"));
    }
}
