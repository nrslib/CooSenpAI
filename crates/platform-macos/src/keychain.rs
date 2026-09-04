use coosenpai_core::ports::{PortError, ProviderApiKeyStore};
use coosenpai_core::provider::ProviderName;
use core_foundation::base::{CFType, TCFType};
use core_foundation::base::{CFTypeRef, OSStatus};
use core_foundation::boolean::CFBoolean;
use core_foundation::data::CFData;
use core_foundation::dictionary::CFDictionary;
use core_foundation::dictionary::CFDictionaryRef;
use core_foundation::string::CFString;
use std::ptr;

const SERVICE: &str = "dev.nrslib.coosenpai.provider-api-key";
const ERR_SEC_ITEM_NOT_FOUND: OSStatus = -25_300;
const ERR_SEC_DUPLICATE_ITEM: OSStatus = -25_299;

#[link(name = "Security", kind = "framework")]
extern "C" {
    fn SecItemAdd(attributes: CFDictionaryRef, result: *mut CFTypeRef) -> OSStatus;
    fn SecItemCopyMatching(query: CFDictionaryRef, result: *mut CFTypeRef) -> OSStatus;
    fn SecItemDelete(query: CFDictionaryRef) -> OSStatus;
    fn SecItemUpdate(query: CFDictionaryRef, attributes: CFDictionaryRef) -> OSStatus;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct MacKeychain;

impl ProviderApiKeyStore for MacKeychain {
    fn read(&self, provider: ProviderName) -> Result<Option<String>, PortError> {
        let query = query(provider, true);
        let mut result: CFTypeRef = ptr::null();
        let status = unsafe {
            SecItemCopyMatching(query.as_concrete_TypeRef(), &mut result as *mut CFTypeRef)
        };
        if status == ERR_SEC_ITEM_NOT_FOUND {
            return Ok(None);
        }
        if status != 0 || result.is_null() {
            return Err(keychain_error("読み込み"));
        }

        let value = unsafe { CFType::wrap_under_create_rule(result) };
        let Some(data) = value.downcast::<CFData>() else {
            return Err(keychain_error("読み込み"));
        };
        String::from_utf8(data.bytes().to_owned())
            .map(Some)
            .map_err(|_| keychain_error("読み込み"))
    }

    fn write(&self, provider: ProviderName, api_key: &str) -> Result<(), PortError> {
        if api_key.is_empty() || api_key.contains('\0') {
            return Err(PortError::Unavailable(
                "API キーは空欄または無効な文字を含められません".to_owned(),
            ));
        }
        let data = CFData::from_buffer(api_key.as_bytes());
        let attributes = attributes(provider, &data);
        let status = unsafe { SecItemAdd(attributes.as_concrete_TypeRef(), ptr::null_mut()) };
        if status == 0 {
            return Ok(());
        }
        if status != ERR_SEC_DUPLICATE_ITEM {
            return Err(keychain_error("保存"));
        }

        let update =
            CFDictionary::from_CFType_pairs(&[(CFString::new("v_Data"), data.as_CFType())]);
        let status = unsafe {
            SecItemUpdate(
                query(provider, false).as_concrete_TypeRef(),
                update.as_concrete_TypeRef(),
            )
        };
        (status == 0)
            .then_some(())
            .ok_or_else(|| keychain_error("保存"))
    }

    fn delete(&self, provider: ProviderName) -> Result<(), PortError> {
        let query = query(provider, false);
        let status = unsafe { SecItemDelete(query.as_concrete_TypeRef()) };
        if status == 0 || status == ERR_SEC_ITEM_NOT_FOUND {
            Ok(())
        } else {
            Err(keychain_error("削除"))
        }
    }
}

fn query(provider: ProviderName, return_data: bool) -> CFDictionary<CFString, CFType> {
    let mut pairs = vec![
        (CFString::new("class"), text("genp")),
        (CFString::new("svce"), text(SERVICE)),
        (CFString::new("acct"), text(provider.as_str())),
    ];
    if return_data {
        pairs.push((CFString::new("r_Data"), CFBoolean::true_value().as_CFType()));
        pairs.push((CFString::new("m_Limit"), text("l_One")));
    }
    CFDictionary::from_CFType_pairs(&pairs)
}

fn attributes(provider: ProviderName, data: &CFData) -> CFDictionary<CFString, CFType> {
    CFDictionary::from_CFType_pairs(&[
        (CFString::new("class"), text("genp")),
        (CFString::new("svce"), text(SERVICE)),
        (CFString::new("acct"), text(provider.as_str())),
        (CFString::new("v_Data"), data.as_CFType()),
    ])
}

fn text(value: &str) -> CFType {
    CFString::new(value).into_CFType()
}

fn keychain_error(operation: &str) -> PortError {
    PortError::Unavailable(format!(
        "macOS キーチェーンから API キーを{operation}できません"
    ))
}
