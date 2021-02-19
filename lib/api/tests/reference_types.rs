use anyhow::Result;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use wasmer::*;

#[test]
fn func_ref_passed_and_returned() -> Result<()> {
    let store = Store::default();
    let wat = r#"(module
    (import "env" "func_ref_identity" (func (param funcref) (result funcref)))
    (type $ret_i32_ty (func (result i32)))
    (table $table (export "table") 2 2 funcref)

    (func (export "run") (param) (result funcref)
          (call 0 (ref.null func)))
    (func (export "call_set_value") (param $fr funcref) (result i32)
          (table.set $table (i32.const 0) (local.get $fr))
          (call_indirect $table (type $ret_i32_ty) (i32.const 0)))
)"#;
    let module = Module::new(&store, wat)?;
    let imports = imports! {
        "env" => {
            "func_ref_identity" => Function::new(&store, FunctionType::new([Type::FuncRef], [Type::FuncRef]), |values| -> Result<Vec<_>, _> {
                Ok(vec![values[0].clone()])
            })
        },
    };

    let instance = Instance::new(&module, &imports)?;

    let f: &Function = instance.exports.get_function("run")?;
    let results = f.call(&[]).unwrap();
    if let Value::FuncRef(fr) = &results[0] {
        assert!(fr.is_none());
    } else {
        panic!("funcref not found!");
    }

    #[derive(Clone, Debug, WasmerEnv)]
    pub struct Env(Arc<AtomicBool>);
    let env = Env(Arc::new(AtomicBool::new(false)));

    let func_to_call = Function::new_native_with_env(&store, env.clone(), |env: &Env| -> i32 {
        env.0.store(true, Ordering::SeqCst);
        343
    });
    let call_set_value: &Function = instance.exports.get_function("call_set_value")?;
    let results: Box<[Value]> = call_set_value.call(&[Value::FuncRef(Some(func_to_call))])?;
    assert!(env.0.load(Ordering::SeqCst));
    assert_eq!(&*results, &[Value::I32(343)]);

    Ok(())
}

#[test]
fn extern_ref_passed_and_returned() -> Result<()> {
    let store = Store::default();
    let wat = r#"(module
    (func $extern_ref_identity (import "env" "extern_ref_identity") (param externref) (result externref))
    (func $extern_ref_identity_native (import "env" "extern_ref_identity_native") (param externref) (result externref))
    (func $get_new_extern_ref (import "env" "get_new_extern_ref") (result externref))
    (func $get_new_extern_ref_native (import "env" "get_new_extern_ref_native") (result externref))

    (func (export "run") (param) (result externref)
          (call $extern_ref_identity (ref.null extern)))
    (func (export "run_native") (param) (result externref)
          (call $extern_ref_identity_native (ref.null extern)))
    (func (export "get_hashmap") (param) (result externref)
          (call $get_new_extern_ref))
    (func (export "get_hashmap_native") (param) (result externref)
          (call $get_new_extern_ref_native))
)"#;
    let module = Module::new(&store, wat)?;
    let imports = imports! {
        "env" => {
            "extern_ref_identity" => Function::new(&store, FunctionType::new([Type::ExternRef], [Type::ExternRef]), |values| -> Result<Vec<_>, _> {
                Ok(vec![values[0].clone()])
            }),
            "extern_ref_identity_native" => Function::new_native(&store, |er: ExternRef<usize>| -> ExternRef<usize> {
                er
            }),
            "get_new_extern_ref" => Function::new(&store, FunctionType::new([], [Type::ExternRef]), |_| -> Result<Vec<_>, _> {
                let inner =
                    [("hello".to_string(), "world".to_string()),
                     ("color".to_string(), "orange".to_string())]
                    .iter()
                    .cloned()
                    .collect::<HashMap<String, String>>();
                let new_extern_ref = VMExternRef::new(inner);
                Ok(vec![Value::ExternRef(new_extern_ref)])
            }),
            "get_new_extern_ref_native" => Function::new_native(&store, || -> ExternRef<HashMap<String, String>> {
                let inner =
                    [("hello".to_string(), "world".to_string()),
                     ("color".to_string(), "orange".to_string())]
                    .iter()
                    .cloned()
                    .collect::<HashMap<String, String>>();
                ExternRef::new(inner)
            })
        },
    };

    let instance = Instance::new(&module, &imports)?;
    for run in &["run", "run_native"] {
        let f: &Function = instance.exports.get_function(run)?;
        let results = f.call(&[]).unwrap();
        if let Value::ExternRef(er) = results[0] {
            assert!(er.is_null());
        } else {
            panic!("result is not an extern ref!");
        }

        let f: NativeFunc<(), ExternRef<usize>> = instance.exports.get_native_function(run)?;
        let result: ExternRef<usize> = f.call()?;
        assert!(result.is_null());
    }

    for get_hashmap in &["get_hashmap", "get_hashmap_native"] {
        let f: &Function = instance.exports.get_function(get_hashmap)?;
        let results = f.call(&[]).unwrap();
        if let Value::ExternRef(er) = results[0] {
            let inner: &HashMap<String, String> = er.downcast().unwrap();
            assert_eq!(inner["hello"], "world");
            assert_eq!(inner["color"], "orange");
        } else {
            panic!("result is not an extern ref!");
        }

        let f: NativeFunc<(), ExternRef<HashMap<String, String>>> =
            instance.exports.get_native_function(get_hashmap)?;

        let result: ExternRef<HashMap<String, String>> = f.call()?;
        let inner: &HashMap<String, String> = result.downcast().unwrap();
        assert_eq!(inner["hello"], "world");
        assert_eq!(inner["color"], "orange");
    }

    Ok(())
}