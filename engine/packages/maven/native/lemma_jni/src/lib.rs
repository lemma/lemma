//! JNI bridge for `com.lemmabase.lemma.Native`.

mod error_json;

use error_json::engine_errors_json;
use indexmap::IndexMap;
use jni::objects::{
    JByteArray, JClass, JObject, JObjectArray, JString, JThrowable, JValue, JValueOwned,
};
use jni::sys::{jboolean, jbyteArray, jint, jlong, jstring, JNI_FALSE};
use jni::{jni_sig, jni_str, Env, EnvUnowned};
use lemma::{DateTimeValue, Engine, EvaluateDataValue, ResourceLimits, SourceType};
use std::sync::Mutex;

type EngineHandle = Mutex<Engine>;

#[derive(Debug)]
enum BridgeError {
    Bug(String),
    Jni(jni::errors::Error),
}

impl From<jni::errors::Error> for BridgeError {
    fn from(error: jni::errors::Error) -> Self {
        Self::Jni(error)
    }
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bug(message) => write!(f, "{message}"),
            Self::Jni(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for BridgeError {}

struct ThrowLemmaBugAndDefault;

impl<T: Default> jni::errors::ErrorPolicy<T, BridgeError> for ThrowLemmaBugAndDefault {
    type Captures<'unowned_env_local: 'native_method, 'native_method> = ();

    fn on_error<'unowned_env_local: 'native_method, 'native_method>(
        env: &mut Env<'unowned_env_local>,
        _: &mut Self::Captures<'unowned_env_local, 'native_method>,
        err: BridgeError,
    ) -> jni::errors::Result<T> {
        if env.exception_check() {
            return Ok(T::default());
        }
        let message = match err {
            BridgeError::Bug(message) => message,
            BridgeError::Jni(error) => format!("BUG: JNI error: {error}"),
        };
        throw_bug(env, &message);
        Ok(T::default())
    }

    fn on_panic<'unowned_env_local: 'native_method, 'native_method>(
        env: &mut Env<'unowned_env_local>,
        _: &mut Self::Captures<'unowned_env_local, 'native_method>,
        _: Box<dyn std::any::Any + Send + 'static>,
    ) -> jni::errors::Result<T> {
        if env.exception_check() {
            return Ok(T::default());
        }
        throw_bug(env, "BUG: Rust panic crossed JNI boundary");
        Ok(T::default())
    }
}

fn throw_lemma_exception(env: &mut Env, message: &str, errors_json: &str) {
    let exception = env
        .find_class(jni_str!("com/lemmabase/lemma/LemmaException"))
        .expect("BUG: LemmaException class must exist");
    let msg = env
        .new_string(message)
        .expect("BUG: failed to allocate exception message");
    let errors = env
        .new_string(errors_json)
        .expect("BUG: failed to allocate errors JSON");
    let obj = env
        .new_object(
            exception,
            jni_sig!("(Ljava/lang/String;Ljava/lang/String;)V"),
            &[JValue::Object(&msg), JValue::Object(&errors)],
        )
        .expect("BUG: failed to construct LemmaException");
    let throwable = env
        .cast_local::<JThrowable>(obj)
        .expect("BUG: LemmaException must be throwable");
    // jni 0.22: throw returns Err(JavaException) after a successful throw.
    match env.throw(throwable) {
        Err(jni::errors::Error::JavaException) => {}
        Ok(()) => panic!("BUG: JNI throw returned Ok without a pending exception"),
        Err(error) => panic!("BUG: JNI throw failed: {error}"),
    }
}

fn throw_bug(env: &mut Env, message: &str) {
    let exception = env
        .find_class(jni_str!("com/lemmabase/lemma/LemmaBugError"))
        .expect("BUG: LemmaBugError class must exist");
    let msg = env
        .new_string(message)
        .expect("BUG: failed to allocate bug message");
    let obj = env
        .new_object(
            exception,
            jni_sig!("(Ljava/lang/String;)V"),
            &[JValue::Object(&msg)],
        )
        .expect("BUG: failed to construct LemmaBugError");
    let throwable = env
        .cast_local::<JThrowable>(obj)
        .expect("BUG: LemmaBugError must be throwable");
    match env.throw(throwable) {
        Err(jni::errors::Error::JavaException) => {}
        Ok(()) => panic!("BUG: JNI throw returned Ok without a pending exception"),
        Err(error) => panic!("BUG: JNI throw failed: {error}"),
    }
}

fn with_catch<'local, R, F>(unowned: &mut EnvUnowned<'local>, f: F) -> R
where
    R: Default,
    F: FnOnce(&mut Env<'local>) -> Result<R, String>,
{
    unowned
        .with_env(|env| f(env).map_err(BridgeError::Bug))
        .resolve::<ThrowLemmaBugAndDefault>()
}

fn handle_from_jlong(handle: jlong) -> Result<&'static EngineHandle, String> {
    if handle == 0 {
        return Err("BUG: engine handle is null (use-after-close or never created)".to_string());
    }
    Ok(unsafe { &*(handle as *const EngineHandle) })
}

fn jstring_required(env: &Env, value: &JString) -> Result<String, String> {
    value
        .try_to_string(env)
        .map_err(|e| format!("BUG: failed to read Java string: {e}"))
}

fn jstring_optional(env: &Env, value: &JString) -> Result<Option<String>, String> {
    if value.is_null() {
        return Ok(None);
    }
    let s = jstring_required(env, value)?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

fn jstring_from_object<'local>(env: &Env, obj: JObject<'local>) -> Result<JString<'local>, String> {
    env.cast_local::<JString>(obj)
        .map_err(|e| format!("BUG: expected java.lang.String: {e}"))
}

/// Run-data walk failure: bad caller input (request Error) or a JNI bridge failure (bug path).
enum WalkError {
    Request(lemma::Error),
    Bridge(String),
}

impl From<String> for WalkError {
    fn from(message: String) -> Self {
        Self::Bridge(message)
    }
}

fn bridge(what: &'static str) -> impl FnOnce(jni::errors::Error) -> String {
    move |e| format!("BUG: JNI {what}: {e}")
}

fn java_map_entries<'local>(
    env: &mut Env<'local>,
    map: &JObject,
) -> Result<Vec<(JObject<'local>, JObject<'local>)>, String> {
    let entry_set = env
        .call_method(
            map,
            jni_str!("entrySet"),
            jni_sig!("()Ljava/util/Set;"),
            &[],
        )
        .map_err(bridge("Map.entrySet"))?
        .l()
        .map_err(bridge("Map.entrySet object"))?;
    let iterator = env
        .call_method(
            &entry_set,
            jni_str!("iterator"),
            jni_sig!("()Ljava/util/Iterator;"),
            &[],
        )
        .map_err(bridge("Set.iterator"))?
        .l()
        .map_err(bridge("Set.iterator object"))?;
    env.delete_local_ref(entry_set);
    let mut entries = Vec::new();
    loop {
        let has_next = env
            .call_method(&iterator, jni_str!("hasNext"), jni_sig!("()Z"), &[])
            .map_err(bridge("Iterator.hasNext"))?
            .z()
            .map_err(bridge("Iterator.hasNext bool"))?;
        if !has_next {
            break;
        }
        let entry = env
            .call_method(
                &iterator,
                jni_str!("next"),
                jni_sig!("()Ljava/lang/Object;"),
                &[],
            )
            .map_err(bridge("Iterator.next"))?
            .l()
            .map_err(bridge("Iterator.next object"))?;
        let key = env
            .call_method(
                &entry,
                jni_str!("getKey"),
                jni_sig!("()Ljava/lang/Object;"),
                &[],
            )
            .map_err(bridge("Map.Entry.getKey"))?
            .l()
            .map_err(bridge("Map.Entry.getKey object"))?;
        let value = env
            .call_method(
                &entry,
                jni_str!("getValue"),
                jni_sig!("()Ljava/lang/Object;"),
                &[],
            )
            .map_err(bridge("Map.Entry.getValue"))?
            .l()
            .map_err(bridge("Map.Entry.getValue object"))?;
        env.delete_local_ref(entry);
        entries.push((key, value));
    }
    env.delete_local_ref(iterator);
    Ok(entries)
}

fn java_map_key(env: &mut Env, key: &JObject) -> Result<String, WalkError> {
    if key.is_null() {
        return Err(WalkError::Request(lemma::Error::request(
            "data map keys must not be null",
            None::<String>,
        )));
    }
    let class_name = java_class_name(env, key)?;
    if class_name != "java.lang.String" {
        return Err(WalkError::Request(lemma::Error::request(
            format!("data map keys must be String, got {class_name}"),
            None::<String>,
        )));
    }
    Ok(call_to_string(env, key)?)
}

/// Top-level `data` map. Outer `Err` is the JNI bug path; inner `Err` are request Errors.
fn java_map_to_evaluate_data(
    env: &mut Env,
    map: &JObject,
) -> Result<Result<IndexMap<String, EvaluateDataValue>, Vec<lemma::Error>>, String> {
    if map.is_null() {
        return Ok(Ok(IndexMap::new()));
    }
    if !instance_of(env, map, jni_str!("java/util/Map"))? {
        return Ok(Err(vec![lemma::Error::request(
            "data must be a Map",
            None::<String>,
        )]));
    }
    let mut entries = Vec::new();
    let mut failures = Vec::new();
    for (key_obj, val_obj) in java_map_entries(env, map)? {
        let key = java_map_key(env, &key_obj);
        env.delete_local_ref(key_obj);
        let key = match key {
            Ok(key) => key,
            Err(WalkError::Request(err)) => {
                failures.push(err);
                continue;
            }
            Err(WalkError::Bridge(message)) => return Err(message),
        };
        if val_obj.is_null() {
            env.delete_local_ref(val_obj);
            failures.push(
                lemma::Error::request(
                    format!("data value for '{key}' must not be null"),
                    Some("omit the key to leave the data unbound"),
                )
                .with_related_data(key),
            );
            continue;
        }
        match java_value_to_evaluate_data(env, val_obj) {
            Ok(parsed) => entries.push((key, parsed)),
            Err(WalkError::Request(err)) => {
                failures.push(lemma::attribute_evaluate_data_error(err, &key))
            }
            Err(WalkError::Bridge(message)) => return Err(message),
        }
    }
    if !failures.is_empty() {
        return Ok(Err(failures));
    }
    Ok(lemma::evaluate_data_from_entries(entries).map_err(|e| vec![e]))
}

/// Non-null Java value to run data. Nested `null` map values are omitted; a `null` list item is a
/// request error.
fn java_value_to_evaluate_data<'local>(
    env: &mut Env<'local>,
    obj: JObject<'local>,
) -> Result<EvaluateDataValue, WalkError> {
    let mut frames: Vec<JavaFrame<'local>> = Vec::new();
    let mut step = JavaStep::Descend(obj);
    loop {
        step = match step {
            JavaStep::Descend(obj) => java_descend(env, obj, &mut frames)?,
            JavaStep::NextChild => {
                let frame = frames
                    .last_mut()
                    .expect("BUG: java frame stack empty while reading a child");
                match frame.next_child(env)? {
                    Some(child) => JavaStep::Descend(child),
                    None => {
                        let frame = frames
                            .pop()
                            .expect("BUG: java frame stack empty on completion");
                        JavaStep::Deliver(frame.finish(env)?)
                    }
                }
            }
            JavaStep::Deliver(value) => match frames.last_mut() {
                None => return Ok(value),
                Some(frame) => {
                    frame.push(value);
                    JavaStep::NextChild
                }
            },
        };
    }
}

enum JavaStep<'local> {
    Descend(JObject<'local>),
    NextChild,
    Deliver(EvaluateDataValue),
}

enum JavaFrame<'local> {
    List {
        list: JObject<'local>,
        size: i32,
        index: i32,
        out: Vec<EvaluateDataValue>,
    },
    Map {
        entries: std::vec::IntoIter<(JObject<'local>, JObject<'local>)>,
        pending_key: Option<String>,
        out: Vec<(String, EvaluateDataValue)>,
    },
}

impl<'local> JavaFrame<'local> {
    fn next_child(&mut self, env: &mut Env<'local>) -> Result<Option<JObject<'local>>, WalkError> {
        match self {
            JavaFrame::List {
                list, size, index, ..
            } => {
                if *index >= *size {
                    return Ok(None);
                }
                let position =
                    usize::try_from(*index).expect("BUG: List index is non-negative") + 1;
                let child = env
                    .call_method(
                        &*list,
                        jni_str!("get"),
                        jni_sig!("(I)Ljava/lang/Object;"),
                        &[JValue::Int(*index)],
                    )
                    .map_err(bridge("List.get"))?
                    .l()
                    .map_err(bridge("List.get object"))?;
                *index += 1;
                if child.is_null() {
                    env.delete_local_ref(child);
                    return Err(WalkError::Request(lemma::evaluate_data_null_list_item(
                        position,
                    )));
                }
                Ok(Some(child))
            }
            JavaFrame::Map {
                entries,
                pending_key,
                ..
            } => {
                for (key_obj, value_obj) in entries.by_ref() {
                    let key = java_map_key(env, &key_obj);
                    env.delete_local_ref(key_obj);
                    let key = key?;
                    if value_obj.is_null() {
                        env.delete_local_ref(value_obj);
                        continue;
                    }
                    *pending_key = Some(key);
                    return Ok(Some(value_obj));
                }
                Ok(None)
            }
        }
    }

    fn push(&mut self, value: EvaluateDataValue) {
        match self {
            JavaFrame::List { out, .. } => out.push(value),
            JavaFrame::Map {
                pending_key, out, ..
            } => {
                let key = pending_key
                    .take()
                    .expect("BUG: java map child delivered without its key");
                out.push((key, value));
            }
        }
    }

    fn finish(self, env: &mut Env<'local>) -> Result<EvaluateDataValue, WalkError> {
        match self {
            JavaFrame::List { list, out, .. } => {
                env.delete_local_ref(list);
                Ok(EvaluateDataValue::List(out))
            }
            JavaFrame::Map { out, .. } => {
                lemma::evaluate_data_map_from_entries(out).map_err(WalkError::Request)
            }
        }
    }
}

fn java_descend<'local>(
    env: &mut Env<'local>,
    obj: JObject<'local>,
    frames: &mut Vec<JavaFrame<'local>>,
) -> Result<JavaStep<'local>, WalkError> {
    if obj.is_null() {
        panic!("BUG: null reached java_value_to_evaluate_data; callers omit or reject null values");
    }
    let class_name = java_class_name(env, &obj)?;
    let value = match class_name.as_str() {
        "java.lang.String" => EvaluateDataValue::Literal(call_to_string(env, &obj)?),
        "java.lang.Boolean" => EvaluateDataValue::Boolean(call_bool(env, &obj)?),
        "java.lang.Float" | "java.lang.Double" => {
            let text = call_to_string(env, &obj)?;
            return Err(WalkError::Request(lemma::Error::request(
                format!("pass as string: \"{text}\""),
                Some("Java float/double has no exact native type; use String, BigDecimal, or an integer type"),
            )));
        }
        "java.math.BigDecimal" => EvaluateDataValue::Literal(call_to_plain_string(env, &obj)?),
        "java.lang.Integer"
        | "java.lang.Long"
        | "java.lang.Short"
        | "java.lang.Byte"
        | "java.math.BigInteger" => EvaluateDataValue::Literal(call_to_string(env, &obj)?),
        _ => return java_open_container(env, obj, &class_name, frames),
    };
    env.delete_local_ref(obj);
    Ok(JavaStep::Deliver(value))
}

fn java_open_container<'local>(
    env: &mut Env<'local>,
    obj: JObject<'local>,
    class_name: &str,
    frames: &mut Vec<JavaFrame<'local>>,
) -> Result<JavaStep<'local>, WalkError> {
    if instance_of(env, &obj, jni_str!("java/util/Map"))? {
        let entries = java_map_entries(env, &obj)?;
        env.delete_local_ref(obj);
        frames.push(JavaFrame::Map {
            entries: entries.into_iter(),
            pending_key: None,
            out: Vec::new(),
        });
        return Ok(JavaStep::NextChild);
    }
    if instance_of(env, &obj, jni_str!("java/util/List"))? {
        let size = call_int_size(env, &obj)?;
        frames.push(JavaFrame::List {
            list: obj,
            size,
            index: 0,
            out: Vec::with_capacity(usize::try_from(size).expect("BUG: List.size is non-negative")),
        });
        return Ok(JavaStep::NextChild);
    }
    Err(WalkError::Request(lemma::Error::request(
        format!("unsupported data value type: {class_name}"),
        Some("use String, Boolean, an integer type, BigDecimal, Map, or List"),
    )))
}

fn instance_of(env: &mut Env, obj: &JObject, class: &jni::strings::JNIStr) -> Result<bool, String> {
    let class = env.find_class(class).map_err(bridge("find_class"))?;
    let is_instance = env
        .is_instance_of(obj, &class)
        .map_err(bridge("is_instance_of"))?;
    env.delete_local_ref(class);
    Ok(is_instance)
}

fn call_bool(env: &mut Env, obj: &JObject) -> Result<bool, String> {
    env.call_method(obj, jni_str!("booleanValue"), jni_sig!("()Z"), &[])
        .map_err(bridge("Boolean.booleanValue"))?
        .z()
        .map_err(bridge("Boolean.booleanValue bool"))
}

fn call_int_size(env: &mut Env, obj: &JObject) -> Result<i32, String> {
    env.call_method(obj, jni_str!("size"), jni_sig!("()I"), &[])
        .map_err(bridge("List.size"))?
        .i()
        .map_err(bridge("List.size int"))
}

fn jstring_from_call(env: &mut Env, raw: JValueOwned) -> Result<String, String> {
    let obj = raw.l().map_err(bridge("string method object"))?;
    let js = env
        .cast_local::<JString>(obj)
        .map_err(bridge("string method cast"))?;
    jstring_required(env, &js)
}

fn call_to_string(env: &mut Env, obj: &JObject) -> Result<String, String> {
    let raw = env
        .call_method(
            obj,
            jni_str!("toString"),
            jni_sig!("()Ljava/lang/String;"),
            &[],
        )
        .map_err(bridge("Object.toString"))?;
    jstring_from_call(env, raw)
}

fn call_to_plain_string(env: &mut Env, obj: &JObject) -> Result<String, String> {
    let raw = env
        .call_method(
            obj,
            jni_str!("toPlainString"),
            jni_sig!("()Ljava/lang/String;"),
            &[],
        )
        .map_err(bridge("BigDecimal.toPlainString"))?;
    jstring_from_call(env, raw)
}

fn java_class_name(env: &mut Env, obj: &JObject) -> Result<String, String> {
    let class = env
        .get_object_class(obj)
        .map_err(bridge("get_object_class"))?;
    let raw = env
        .call_method(
            &class,
            jni_str!("getName"),
            jni_sig!("()Ljava/lang/String;"),
            &[],
        )
        .map_err(bridge("Class.getName"))?;
    env.delete_local_ref(class);
    jstring_from_call(env, raw)
}

fn string_pairs_from_java(
    env: &mut Env,
    keys: &JObjectArray,
    values: &JObjectArray,
) -> Result<Vec<(String, String)>, String> {
    let len = keys
        .len(env)
        .map_err(|e| format!("BUG: get_array_length keys: {e}"))?;
    let values_len = values
        .len(env)
        .map_err(|e| format!("BUG: get_array_length values: {e}"))?;
    if len != values_len {
        return Err("BUG: data key/value array length mismatch".to_string());
    }
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let key_obj = keys
            .get_element(env, i)
            .map_err(|e| format!("BUG: get key[{i}]: {e}"))?;
        let val_obj = values
            .get_element(env, i)
            .map_err(|e| format!("BUG: get value[{i}]: {e}"))?;
        let key = jstring_required(env, &jstring_from_object(env, key_obj)?)?;
        let value = jstring_required(env, &jstring_from_object(env, val_obj)?)?;
        out.push((key, value));
    }
    Ok(out)
}

fn limits_from_json(raw: &str) -> Result<ResourceLimits, lemma::Error> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| lemma::Error::request(format!("limits JSON: {e}"), None::<String>))?;
    let obj = value
        .as_object()
        .ok_or_else(|| lemma::Error::request("limits must be a JSON object", None::<String>))?;
    let mut limits = ResourceLimits::default();
    for (key, v) in obj {
        let n = v.as_u64().ok_or_else(|| {
            lemma::Error::request(
                format!("limits value for '{key}' must be a non-negative integer"),
                None::<String>,
            )
        })? as usize;
        limits
            .apply(key, n)
            .map_err(|e| lemma::Error::request(e, None::<String>))?;
    }
    Ok(limits)
}

fn parse_effective(env: &mut Env, effective: &JString) -> Result<Option<DateTimeValue>, ()> {
    let raw = match jstring_optional(env, effective) {
        Ok(v) => v,
        Err(message) => {
            throw_bug(env, &message);
            return Err(());
        }
    };
    let Some(raw) = raw else {
        return Ok(None);
    };
    match raw.parse::<DateTimeValue>() {
        Ok(dt) => Ok(Some(dt)),
        Err(e) => {
            let err = lemma::Error::request(format!("Invalid effective date: {e}"), None::<String>);
            throw_lemma_exception(
                env,
                "invalid effective date",
                &engine_errors_json(std::slice::from_ref(&err)),
            );
            Err(())
        }
    }
}

fn return_string(env: &mut Env, value: String) -> jstring {
    env.new_string(value)
        .expect("BUG: failed to allocate return string")
        .into_raw()
}

fn bytes_from_jbyte_array(env: &Env, array: &JByteArray) -> Result<Vec<u8>, String> {
    if array.is_null() {
        return Err("BUG: snapshot bytes array is null".to_string());
    }
    env.convert_byte_array(array)
        .map_err(|e| format!("BUG: failed to read Java byte array: {e}"))
}

fn return_bytes(env: &mut Env, bytes: &[u8]) -> Result<jbyteArray, String> {
    env.byte_array_from_slice(bytes)
        .map(|arr| arr.into_raw())
        .map_err(|e| format!("BUG: failed to allocate Java byte array: {e}"))
}

#[no_mangle]
pub extern "system" fn Java_com_lemmabase_lemma_Native_version(
    mut unowned: EnvUnowned,
    _class: JClass,
) -> jstring {
    with_catch(&mut unowned, |env| {
        Ok(return_string(env, env!("CARGO_PKG_VERSION").to_string()))
    })
}

#[no_mangle]
pub extern "system" fn Java_com_lemmabase_lemma_Native_create(
    mut unowned: EnvUnowned,
    _class: JClass,
) -> jlong {
    with_catch(&mut unowned, |_| {
        let engine = Box::new(Mutex::new(Engine::new()));
        Ok(Box::into_raw(engine) as jlong)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_lemmabase_lemma_Native_createWithLimits(
    mut unowned: EnvUnowned,
    _class: JClass,
    limits_json: JString,
) -> jlong {
    with_catch(&mut unowned, |env| {
        let raw = jstring_required(env, &limits_json)?;
        let limits = match limits_from_json(&raw) {
            Ok(l) => l,
            Err(err) => {
                throw_lemma_exception(
                    env,
                    "invalid resource limits",
                    &engine_errors_json(std::slice::from_ref(&err)),
                );
                return Ok(0);
            }
        };
        let engine = Box::new(Mutex::new(Engine::with_limits(limits)));
        Ok(Box::into_raw(engine) as jlong)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_lemmabase_lemma_Native_fromSnapshot(
    mut unowned: EnvUnowned,
    _class: JClass,
    bytes: JByteArray,
) -> jlong {
    with_catch(&mut unowned, |env| {
        let raw = bytes_from_jbyte_array(env, &bytes)?;
        match Engine::from_snapshot(&raw) {
            Ok(engine) => {
                let handle = Box::new(Mutex::new(engine));
                Ok(Box::into_raw(handle) as jlong)
            }
            Err(err) => {
                throw_lemma_exception(
                    env,
                    "fromSnapshot failed",
                    &engine_errors_json(std::slice::from_ref(&err)),
                );
                Ok(0)
            }
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_lemmabase_lemma_Native_destroy(
    mut unowned: EnvUnowned,
    _class: JClass,
    handle: jlong,
) {
    with_catch(&mut unowned, |_| {
        if handle != 0 {
            unsafe {
                drop(Box::from_raw(handle as *mut EngineHandle));
            }
        }
        Ok(())
    });
}

#[no_mangle]
pub extern "system" fn Java_com_lemmabase_lemma_Native_load(
    mut unowned: EnvUnowned,
    _class: JClass,
    handle: jlong,
    code: JString,
) {
    with_catch(&mut unowned, |env| {
        let engine = handle_from_jlong(handle)?;
        let code = jstring_required(env, &code)?;
        let mut guard = engine
            .lock()
            .map_err(|_| "BUG: Engine lock poisoned".to_string())?;
        if let Err(load_err) = guard.load([(SourceType::Volatile, code)]) {
            throw_lemma_exception(env, "load failed", &engine_errors_json(&load_err.errors));
        }
        Ok(())
    });
}

#[no_mangle]
pub extern "system" fn Java_com_lemmabase_lemma_Native_loadLabeled(
    mut unowned: EnvUnowned,
    _class: JClass,
    handle: jlong,
    labels: JObjectArray,
    codes: JObjectArray,
) {
    with_catch(&mut unowned, |env| {
        let engine = handle_from_jlong(handle)?;
        let pairs = string_pairs_from_java(env, &labels, &codes)?;
        let mut batch = Vec::with_capacity(pairs.len());
        for (label, code) in pairs {
            match SourceType::from_binding_label(&label) {
                Ok(source_type) => batch.push((source_type, code)),
                Err(e) => {
                    let err = lemma::Error::request(
                        format!("load: label '{label}': {e}"),
                        None::<String>,
                    );
                    throw_lemma_exception(
                        env,
                        "load failed",
                        &engine_errors_json(std::slice::from_ref(&err)),
                    );
                    return Ok(());
                }
            }
        }
        let mut guard = engine
            .lock()
            .map_err(|_| "BUG: Engine lock poisoned".to_string())?;
        if let Err(load_err) = guard.load(batch) {
            throw_lemma_exception(env, "load failed", &engine_errors_json(&load_err.errors));
        }
        Ok(())
    });
}

#[no_mangle]
pub extern "system" fn Java_com_lemmabase_lemma_Native_specs(
    mut unowned: EnvUnowned,
    _class: JClass,
    handle: jlong,
) -> jstring {
    with_catch(&mut unowned, |env| {
        let engine = handle_from_jlong(handle)?;
        let guard = engine
            .lock()
            .map_err(|_| "BUG: Engine lock poisoned".to_string())?;
        let json = serde_json::to_string(&guard.specs())
            .map_err(|e| format!("BUG: specs serialization failed: {e}"))?;
        Ok(return_string(env, json))
    })
}

#[no_mangle]
pub extern "system" fn Java_com_lemmabase_lemma_Native_show(
    mut unowned: EnvUnowned,
    _class: JClass,
    handle: jlong,
    repository: JString,
    spec: JString,
    effective: JString,
) -> jstring {
    with_catch(&mut unowned, |env| {
        let engine = handle_from_jlong(handle)?;
        let repo = jstring_optional(env, &repository)?;
        let spec = jstring_required(env, &spec)?;
        let effective = match parse_effective(env, &effective) {
            Ok(v) => v,
            Err(()) => return Ok(std::ptr::null_mut()),
        };
        let guard = engine
            .lock()
            .map_err(|_| "BUG: Engine lock poisoned".to_string())?;
        match guard.show(repo.as_deref(), &spec, effective.as_ref()) {
            Ok(view) => {
                let json = serde_json::to_string(&lemma::api::show_document(&guard, &view))
                    .map_err(|e| format!("BUG: show serialization failed: {e}"))?;
                Ok(return_string(env, json))
            }
            Err(err) => {
                throw_lemma_exception(
                    env,
                    "show failed",
                    &engine_errors_json(std::slice::from_ref(&err)),
                );
                Ok(std::ptr::null_mut())
            }
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_lemmabase_lemma_Native_source(
    mut unowned: EnvUnowned,
    _class: JClass,
    handle: jlong,
    repository: JString,
    spec: JString,
    effective: JString,
) -> jstring {
    with_catch(&mut unowned, |env| {
        let engine = handle_from_jlong(handle)?;
        let repo = jstring_optional(env, &repository)?;
        let spec_name = jstring_optional(env, &spec)?;
        let effective = match (&spec_name, parse_effective(env, &effective)) {
            (_, Err(())) => return Ok(std::ptr::null_mut()),
            (Some(_), Ok(v)) => v,
            (None, Ok(_)) => None,
        };
        let guard = engine
            .lock()
            .map_err(|_| "BUG: Engine lock poisoned".to_string())?;
        match guard.source(repo.as_deref(), spec_name.as_deref(), effective.as_ref()) {
            Ok(text) => Ok(return_string(env, text)),
            Err(err) => {
                throw_lemma_exception(
                    env,
                    "source failed",
                    &engine_errors_json(std::slice::from_ref(&err)),
                );
                Ok(std::ptr::null_mut())
            }
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_lemmabase_lemma_Native_run(
    mut unowned: EnvUnowned,
    _class: JClass,
    handle: jlong,
    repository: JString,
    spec: JString,
    effective: JString,
    data_map: JObject,
    rules: JObjectArray,
    explain: jboolean,
) -> jstring {
    with_catch(&mut unowned, |env| {
        let engine = handle_from_jlong(handle)?;
        let repo = jstring_optional(env, &repository)?;
        let spec = jstring_required(env, &spec)?;
        let effective = match parse_effective(env, &effective) {
            Ok(v) => v,
            Err(()) => return Ok(std::ptr::null_mut()),
        };
        let data = match java_map_to_evaluate_data(env, &data_map)? {
            Ok(data) => data,
            Err(errs) => {
                throw_lemma_exception(env, "evaluate failed", &engine_errors_json(&errs));
                return Ok(std::ptr::null_mut());
            }
        };
        let rules = if rules.is_null() {
            None
        } else {
            let len = rules
                .len(env)
                .map_err(|e| format!("BUG: get_array_length rules: {e}"))?;
            if len == 0 {
                let err =
                    lemma::Error::request("rules must not be empty".to_string(), None::<String>);
                throw_lemma_exception(
                    env,
                    "evaluate failed",
                    &engine_errors_json(std::slice::from_ref(&err)),
                );
                return Ok(std::ptr::null_mut());
            }
            let mut names = Vec::with_capacity(len);
            for i in 0..len {
                let obj = rules
                    .get_element(env, i)
                    .map_err(|e| format!("BUG: get rules[{i}]: {e}"))?;
                names.push(jstring_required(env, &jstring_from_object(env, obj)?)?);
            }
            Some(names)
        };
        let explain = explain != JNI_FALSE;
        let guard = engine
            .lock()
            .map_err(|_| "BUG: Engine lock poisoned".to_string())?;
        match guard.evaluate(
            repo.as_deref(),
            &spec,
            effective.as_ref(),
            data,
            rules.as_deref(),
            explain,
        ) {
            Ok(response) => {
                let json = serde_json::to_string(&lemma::api::Response::from(&response))
                    .map_err(|e| format!("BUG: response serialization failed: {e}"))?;
                Ok(return_string(env, json))
            }
            Err(err) => {
                throw_lemma_exception(
                    env,
                    "evaluate failed",
                    &engine_errors_json(std::slice::from_ref(&err)),
                );
                Ok(std::ptr::null_mut())
            }
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_lemmabase_lemma_Native_remove(
    mut unowned: EnvUnowned,
    _class: JClass,
    handle: jlong,
    repository: JString,
    spec: JString,
    effective: JString,
) {
    with_catch(&mut unowned, |env| {
        let engine = handle_from_jlong(handle)?;
        let repo = jstring_optional(env, &repository)?;
        let spec = jstring_required(env, &spec)?;
        let effective = match parse_effective(env, &effective) {
            Ok(v) => v,
            Err(()) => return Ok(()),
        };
        let mut guard = engine
            .lock()
            .map_err(|_| "BUG: Engine lock poisoned".to_string())?;
        if let Err(err) = guard.remove(repo.as_deref(), &spec, effective.as_ref()) {
            throw_lemma_exception(
                env,
                "remove failed",
                &engine_errors_json(std::slice::from_ref(&err)),
            );
        }
        Ok(())
    });
}

#[no_mangle]
pub extern "system" fn Java_com_lemmabase_lemma_Native_update(
    mut unowned: EnvUnowned,
    _class: JClass,
    handle: jlong,
    repository: JString,
    code: JString,
    attribute: JString,
) {
    with_catch(&mut unowned, |env| {
        let engine = handle_from_jlong(handle)?;
        let repo = jstring_optional(env, &repository)?;
        let code = jstring_required(env, &code)?;
        let attribute = jstring_optional(env, &attribute)?;
        let source_type = match attribute
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            None => SourceType::Volatile,
            Some(label) => match SourceType::from_binding_label(label) {
                Ok(st) => st,
                Err(message) => {
                    throw_lemma_exception(
                        env,
                        "update failed",
                        &engine_errors_json(&[lemma::Error::request(
                            format!("update: label '{label}': {message}"),
                            None::<String>,
                        )]),
                    );
                    return Ok(());
                }
            },
        };
        let mut guard = engine
            .lock()
            .map_err(|_| "BUG: Engine lock poisoned".to_string())?;
        if let Err(load_err) = guard.update(repo.as_deref(), code, source_type) {
            throw_lemma_exception(env, "update failed", &engine_errors_json(&load_err.errors));
        }
        Ok(())
    });
}

#[no_mangle]
pub extern "system" fn Java_com_lemmabase_lemma_Native_installStart(
    mut unowned: EnvUnowned,
    _class: JClass,
    repository: JString,
    limits_json: JString,
) -> jlong {
    with_catch(&mut unowned, |env| {
        let repository = jstring_required(env, &repository)?;
        let limits = match jstring_optional(env, &limits_json)? {
            None => lemma::ResourceLimits::default(),
            Some(raw) if raw.is_empty() => lemma::ResourceLimits::default(),
            Some(raw) => limits_from_json(&raw).map_err(|e| e.message().to_string())?,
        };
        let (install, step) = lemma::Install::start(registries(), &repository, limits);
        let handle = Box::into_raw(Box::new(InstallHandle {
            install,
            step: Some(step),
        }));
        Ok(handle as jlong)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_lemmabase_lemma_Native_installStep(
    mut unowned: EnvUnowned,
    _class: JClass,
    handle: jlong,
) -> jstring {
    with_catch(&mut unowned, |env| {
        let install_handle = install_handle_from_jlong(handle)?;
        let step = install_handle
            .step
            .as_ref()
            .ok_or_else(|| "BUG: installStep called with no current step".to_string())?;
        // Java owns the free via installFree; keep the handle until then so a
        // Finished(Err) that throws in Java cannot use-after-free.
        let json = serialize_install_step(step)?;
        Ok(return_string(env, json))
    })
}

#[no_mangle]
pub extern "system" fn Java_com_lemmabase_lemma_Native_installRespond(
    mut unowned: EnvUnowned,
    _class: JClass,
    handle: jlong,
    status: jint,
    headers_json: JString,
    body: JString,
) -> jstring {
    with_catch(&mut unowned, |env| {
        let headers_json = jstring_required(env, &headers_json)?;
        let body = jstring_required(env, &body)?;
        let headers = parse_headers_json(&headers_json)?;
        if status < 0 || status > u16::MAX as i32 {
            return Err(format!("BUG: invalid HTTP status from Java: {status}"));
        }
        let response = Ok(lemma::HttpResponse {
            status: status as u16,
            headers,
            body,
        });
        advance_install(env, handle, response)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_lemmabase_lemma_Native_installFail(
    mut unowned: EnvUnowned,
    _class: JClass,
    handle: jlong,
    message: JString,
) -> jstring {
    with_catch(&mut unowned, |env| {
        let message = jstring_required(env, &message)?;
        let response = Err(lemma::TransportFailure { message });
        advance_install(env, handle, response)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_lemmabase_lemma_Native_installFree(
    mut unowned: EnvUnowned,
    _class: JClass,
    handle: jlong,
) {
    with_catch(&mut unowned, |_| {
        free_install_handle(handle);
        Ok(())
    });
}

fn registries() -> &'static lemma::Registries {
    use std::sync::OnceLock;
    static REGISTRIES: OnceLock<lemma::Registries> = OnceLock::new();
    REGISTRIES.get_or_init(lemma::Registries::default)
}

struct InstallHandle {
    install: lemma::Install<'static>,
    step: Option<lemma::InstallStep>,
}

fn install_handle_from_jlong(handle: jlong) -> Result<&'static mut InstallHandle, String> {
    if handle == 0 {
        return Err("BUG: install handle is null".to_string());
    }
    Ok(unsafe { &mut *(handle as *mut InstallHandle) })
}

fn free_install_handle(handle: jlong) {
    if handle != 0 {
        unsafe {
            drop(Box::from_raw(handle as *mut InstallHandle));
        }
    }
}

fn advance_install(
    env: &mut Env,
    handle: jlong,
    response: Result<lemma::HttpResponse, lemma::TransportFailure>,
) -> Result<jstring, String> {
    let install_handle = install_handle_from_jlong(handle)?;
    if install_handle.step.is_none() {
        return Err("BUG: installRespond/installFail called with no pending step".to_string());
    }
    let next = install_handle.install.respond(response);
    let json = serialize_install_step(&next)?;
    install_handle.step = Some(next);
    Ok(return_string(env, json))
}

fn serialize_install_step(step: &lemma::InstallStep) -> Result<String, String> {
    let value = match step {
        lemma::InstallStep::Fetch(fetch) => serde_json::json!({ "fetch": fetch }),
        lemma::InstallStep::Finished(Ok(result)) => {
            let ok = serde_json::to_value(result)
                .unwrap_or_else(|e| panic!("BUG: RepositoryInstallResult serialize failed: {e}"));
            serde_json::json!({ "finished": { "ok": ok } })
        }
        lemma::InstallStep::Finished(Err(error)) => {
            let err = lemma::EngineError::from(error);
            serde_json::json!({
                "finished": {
                    "err": [err],
                }
            })
        }
    };
    serde_json::to_string(&value)
        .map_err(|e| format!("BUG: install step serialization failed: {e}"))
}

fn parse_headers_json(raw: &str) -> Result<Vec<lemma::Header>, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("BUG: headers JSON parse failed: {e}"))?;
    let arr = value
        .as_array()
        .ok_or_else(|| "BUG: headers JSON must be an array".to_string())?;
    let mut headers = Vec::with_capacity(arr.len());
    for item in arr {
        let obj = item
            .as_object()
            .ok_or_else(|| "BUG: header entry must be an object".to_string())?;
        let name = obj
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "BUG: header missing name".to_string())?;
        let value = obj
            .get("value")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "BUG: header missing value".to_string())?;
        headers.push(lemma::Header {
            name: name.to_string(),
            value: value.to_string(),
        });
    }
    Ok(headers)
}

#[no_mangle]
pub extern "system" fn Java_com_lemmabase_lemma_Native_format(
    mut unowned: EnvUnowned,
    _class: JClass,
    code: JString,
) -> jstring {
    with_catch(&mut unowned, |env| {
        let code = jstring_required(env, &code)?;
        match lemma::format_source(&code, SourceType::Volatile) {
            Ok(formatted) => Ok(return_string(env, formatted)),
            Err(err) => {
                throw_lemma_exception(
                    env,
                    "format failed",
                    &engine_errors_json(std::slice::from_ref(&err)),
                );
                Ok(std::ptr::null_mut())
            }
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_lemmabase_lemma_Native_limits(
    mut unowned: EnvUnowned,
    _class: JClass,
    handle: jlong,
) -> jstring {
    with_catch(&mut unowned, |env| {
        let engine = handle_from_jlong(handle)?;
        let guard = engine
            .lock()
            .map_err(|_| "BUG: Engine lock poisoned".to_string())?;
        let json = serde_json::to_string(guard.limits())
            .map_err(|e| format!("BUG: limits serialization failed: {e}"))?;
        Ok(return_string(env, json))
    })
}

#[no_mangle]
pub extern "system" fn Java_com_lemmabase_lemma_Native_snapshot(
    mut unowned: EnvUnowned,
    _class: JClass,
    handle: jlong,
) -> jbyteArray {
    with_catch(&mut unowned, |env| {
        let engine = handle_from_jlong(handle)?;
        let guard = engine
            .lock()
            .map_err(|_| "BUG: Engine lock poisoned".to_string())?;
        match guard.snapshot() {
            Ok(bytes) => return_bytes(env, &bytes),
            Err(err) => {
                throw_lemma_exception(
                    env,
                    "snapshot failed",
                    &engine_errors_json(std::slice::from_ref(&err)),
                );
                Ok(std::ptr::null_mut())
            }
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_lemmabase_lemma_Native_quality(
    mut unowned: EnvUnowned,
    _class: JClass,
    handle: jlong,
) -> jstring {
    with_catch(&mut unowned, |env| {
        let engine = handle_from_jlong(handle)?;
        let guard = engine
            .lock()
            .map_err(|_| "BUG: Engine lock poisoned".to_string())?;
        let json = serde_json::to_string(&guard.quality())
            .map_err(|e| format!("BUG: quality serialization failed: {e}"))?;
        Ok(return_string(env, json))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_from_json_applies_known_keys() {
        let limits = limits_from_json(r#"{"max_sources": 7, "max_dag_specs": 3}"#)
            .expect("valid limits JSON");
        assert_eq!(limits.max_sources, 7);
        assert_eq!(limits.max_dag_specs, 3);
    }

    #[test]
    fn limits_from_json_rejects_unknown_key() {
        let err = limits_from_json(r#"{"not_a_limit": 1}"#).expect_err("unknown key");
        assert!(err.message().contains("unknown limits key"));
    }

    #[test]
    fn limits_from_json_rejects_non_object() {
        let err = limits_from_json("[]").expect_err("array");
        assert!(err.message().contains("limits must be a JSON object"));
    }
}
