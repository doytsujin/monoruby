use crate::*;

//
// Class class
//

pub(super) fn init(globals: &mut Globals) {
    globals.define_builtin_func(CLASS_CLASS, "superclass", superclass, 0);
    globals.define_builtin_func(CLASS_CLASS, "to_s", tos, 0);
    globals.define_builtin_func(CLASS_CLASS, "constants", constants, 0);
}

/// ### Class#superclass
/// - superclass -> Class | nil
///
/// [https://docs.ruby-lang.org/ja/latest/method/Class/i/superclass.html]
extern "C" fn superclass(
    _vm: &mut Interp,
    globals: &mut Globals,
    arg: Arg,
    _len: usize,
) -> Option<Value> {
    let class_id = arg.self_value().as_class();
    let res = match class_id.super_class(globals) {
        None => Value::nil(),
        Some(super_id) => super_id.get_obj(globals),
    };
    Some(res)
}

/// ### Class#to_s
/// - to_s -> String
///
/// [https://docs.ruby-lang.org/ja/latest/method/Object/i/to_s.html]
extern "C" fn tos(_vm: &mut Interp, globals: &mut Globals, arg: Arg, _len: usize) -> Option<Value> {
    let class_name = arg.self_value().as_class().get_name(globals);
    let res = Value::new_string(class_name.into_bytes());
    Some(res)
}

/// ### Module#constants
/// - constants(inherit = true) -> [Symbol]
///
/// [https://docs.ruby-lang.org/ja/latest/method/Module/i/constants.html]
extern "C" fn constants(
    _vm: &mut Interp,
    globals: &mut Globals,
    arg: Arg,
    _len: usize,
) -> Option<Value> {
    let class_id = arg.self_value().as_class();
    let v = globals
        .get_constant_names(class_id)
        .into_iter()
        .map(|name| Value::new_symbol(name))
        .collect();
    Some(Value::new_array(v))
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_class() {
        run_test("Time.superclass.to_s");
    }
}
