use std::mem::ManuallyDrop;
use crate::FfiBuf;

#[repr(C)]
#[derive(Debug)]
pub(crate) struct FfiUuid {
    a: u64,
    b: u64,
}

impl From<uuid::Uuid> for FfiUuid {
    fn from(uuid: uuid::Uuid) -> Self {
        let (a, b) = uuid.as_u64_pair();
        FfiUuid { a, b }
    }
}

impl From<FfiUuid> for uuid::Uuid {
    fn from(ffi_uuid: FfiUuid) -> Self {
        uuid::Uuid::from_u64_pair(ffi_uuid.a, ffi_uuid.b)
    }
}

impl TryFrom<String> for FfiUuid {
    type Error = uuid::Error;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Ok(uuid::Uuid::parse_str(&s)?.into())
    }
}

impl From<FfiUuid> for String {
    fn from(ffi_uuid: FfiUuid) -> Self {
        ffi_uuid.into()
    }
}

#[no_mangle]
extern "C" fn uuid_new_v4_extern() -> FfiUuid {
    uuid::Uuid::new_v4().into()
}

#[no_mangle]
extern "C" fn uuid_from_string_extern(str: FfiBuf) -> FfiUuid {
    ManuallyDrop::new(str).to_uuid().into()
}

#[no_mangle]
extern "C" fn uuid_to_string_extern(id: FfiUuid) -> FfiBuf {
    FfiBuf::from(uuid::Uuid::from(id).to_string())
}