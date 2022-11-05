use ruruby_parse::{
    BinOp, BlockInfo, Loc, LvarCollector, Node, NodeKind, ParamKind, ParseErr, ParseErrKind,
    Parser, SourceInfoRef,
};
use std::io::Write;
use std::io::{stdout, BufWriter, Stdout};
use std::path::PathBuf;

use super::*;

mod class;
mod error;
mod functions;
pub use class::*;
pub use error::*;
pub use functions::*;

///
/// Global state.
///
pub struct Globals {
    /// function info.
    pub func: FnStore,
    /// class table.
    class: ClassStore,
    error: Option<MonorubyErr>,
    /// warning level.
    pub warning: u8,
    /// suppress jit compilation.
    pub no_jit: bool,
    /// stdout.
    stdout: BufWriter<Stdout>,
}

impl Globals {
    pub fn new(warning: u8, no_jit: bool) -> Self {
        let mut globals = Self {
            func: FnStore::new(),
            class: ClassStore::new(),
            error: None,
            warning,
            no_jit,
            stdout: BufWriter::new(stdout()),
        };
        builtins::init_builtins(&mut globals);
        globals
    }

    pub(crate) fn flush_stdout(&mut self) {
        self.stdout.flush().unwrap();
    }

    pub(crate) fn write_stdout(&mut self, bytes: &[u8]) {
        self.stdout.write_all(bytes).unwrap();
    }

    pub fn exec_startup(&mut self) {
        let path = std::path::Path::new("startup/startup.rb");
        let code = include_str!("../../startup/startup.rb").to_string();
        let startup_fid = match self.compile_script(code, path) {
            Ok(func_id) => func_id,
            Err(err) => {
                eprintln!("error occured in compiling startup.rb.");
                eprintln!("{}", err.get_error_message(self));
                err.show_loc();
                return;
            }
        };
        match Executor::eval_toplevel(self, startup_fid) {
            Ok(_) => {}
            Err(err) => {
                eprintln!("error occured in executing startup.rb.");
                eprintln!("{}", err.get_error_message(self));
                err.show_loc();
            }
        };
    }
}

impl Globals {
    fn array_tos(&self, v: &[Value]) -> String {
        match v.len() {
            0 => "[]".to_string(),
            1 => format!("[{}]", self.val_inspect(v[0])),
            _ => {
                let mut s = format!("[{}", self.val_inspect(v[0]));
                for val in v[1..].iter() {
                    s += &format!(", {}", self.val_inspect(*val));
                }
                s += "]";
                s
            }
        }
    }

    fn object_tos(&self, val: Value) -> String {
        if let Some(name) = self.get_ivar(val, IdentId::_NAME) {
            self.val_tos(name)
        } else {
            format!(
                "#<{}:0x{:016x}>",
                val.class_id().get_name(self),
                val.rvalue().id()
            )
        }
    }

    fn object_inspect(&self, val: Value) -> String {
        if let Some(name) = self.get_ivar(val, IdentId::_NAME) {
            self.val_tos(name)
        } else {
            let mut s = String::new();
            for (id, v) in self.get_ivars(val).into_iter() {
                s += &format!(" {}={}", IdentId::get_name(id), v.to_s(self));
            }
            format!(
                "#<{}:0x{:016x}{s}>",
                val.class_id().get_name(self),
                val.rvalue().id()
            )
        }
    }

    pub(crate) fn val_tos(&self, val: Value) -> String {
        match val.unpack() {
            RV::Nil => "nil".to_string(),
            RV::Bool(b) => format!("{:?}", b),
            RV::Integer(n) => format!("{}", n),
            RV::BigInt(n) => format!("{}", n),
            RV::Float(f) => dtoa::Buffer::new().format(f).to_string(),
            RV::Symbol(id) => IdentId::get_name(id),
            RV::String(s) => match String::from_utf8(s.to_vec()) {
                Ok(s) => s,
                Err(_) => format!("{:?}", s),
            },
            RV::Object(rvalue) => match rvalue.kind() {
                ObjKind::CLASS => rvalue.as_class().get_name(self),
                ObjKind::TIME => rvalue.as_time().to_string(),
                ObjKind::ARRAY => self.array_tos(rvalue.as_array()),
                ObjKind::OBJECT => self.object_tos(val),
                _ => format!("{:016x}", val.get()),
            },
        }
    }

    pub(crate) fn val_tobytes(&self, val: Value) -> Vec<u8> {
        if let RV::String(s) = val.unpack() {
            return s.to_vec();
        }
        self.val_tos(val).into_bytes()
    }

    pub(crate) fn val_inspect(&self, val: Value) -> String {
        match val.unpack() {
            RV::Nil => "nil".to_string(),
            RV::Bool(b) => format!("{:?}", b),
            RV::Integer(n) => format!("{}", n),
            RV::BigInt(n) => format!("{}", n),
            RV::Float(f) => dtoa::Buffer::new().format(f).to_string(),
            RV::Symbol(id) => format!(":{}", IdentId::get_name(id)),
            RV::String(s) => match String::from_utf8(s.to_vec()) {
                Ok(s) => format!("\"{}\"", escape_string::escape(&s)),
                Err(_) => format!("{:?}", s),
            },
            RV::Object(rvalue) => match rvalue.kind() {
                ObjKind::CLASS => rvalue.as_class().get_name(self),
                ObjKind::TIME => rvalue.as_time().to_string(),
                ObjKind::ARRAY => self.array_tos(rvalue.as_array()),
                ObjKind::OBJECT => self.object_inspect(val),
                _ => unreachable!(),
            },
        }
    }

    pub(crate) fn find_method(&mut self, obj: Value, name: IdentId) -> Option<FuncId> {
        let mut class_id = obj.class_id();
        if let Some(func_id) = self.get_method(class_id, name) {
            return Some(func_id);
        }
        while let Some(super_class) = class_id.super_class(self) {
            class_id = super_class;
            if let Some(func_id) = self.get_method(class_id, name) {
                return Some(func_id);
            }
        }
        None
    }

    pub(crate) fn find_method_checked(
        &mut self,
        obj: Value,
        func_name: IdentId,
        args_len: usize,
    ) -> Option<FuncId> {
        let func_id = match self.find_method(obj, func_name) {
            Some(id) => id,
            None => {
                self.err_method_not_found(func_name, obj);
                return None;
            }
        };
        self.check_arg(func_id, args_len)?;
        Some(func_id)
    }

    pub(crate) fn check_arg(&mut self, func_id: FuncId, args_len: usize) -> Option<()> {
        let arity = self.func[func_id].arity();
        if arity != -1 && (arity as usize) != args_len {
            self.error = Some(MonorubyErr::wrong_arguments(arity as usize, args_len));
            return None;
        }
        Some(())
    }

    pub(crate) fn define_builtin_func(
        &mut self,
        class_id: ClassId,
        name: &str,
        address: BuiltinFn,
        arity: i32,
    ) -> FuncId {
        let func_id = self.func.add_builtin_func(name.to_string(), address, arity);
        let name_id = IdentId::get_ident_id(name);
        self.add_method(class_id, name_id, func_id);
        func_id
    }

    pub(crate) fn define_builtin_singleton_func(
        &mut self,
        class_id: ClassId,
        name: &str,
        address: BuiltinFn,
        arity: i32,
    ) -> FuncId {
        let class_id = self.get_singleton_id(class_id);
        let func_id = self.func.add_builtin_func(name.to_string(), address, arity);
        let name_id = IdentId::get_ident_id(name);
        self.add_method(class_id, name_id, func_id);
        func_id
    }

    ///
    /// Define attribute reader for *class_id* and *ivar_name*.
    ///
    pub(crate) fn define_attr_reader(
        &mut self,
        interp: &mut Executor,
        class_id: ClassId,
        method_name: IdentId,
    ) -> IdentId {
        let ivar_name = IdentId::add_ivar_prefix(method_name);
        let method_name_str = IdentId::get_name(method_name);
        let func_id = self.func.add_attr_reader(method_name_str, ivar_name);
        self.add_method(class_id, method_name, func_id);
        interp.class_version_inc();
        method_name
    }

    ///
    /// Define attribute writer for *class_id* and *ivar_name*.
    ///
    pub(crate) fn define_attr_writer(
        &mut self,
        interp: &mut Executor,
        class_id: ClassId,
        method_name: IdentId,
    ) -> IdentId {
        let ivar_name = IdentId::add_ivar_prefix(method_name);
        let method_name = IdentId::add_assign_postfix(method_name);
        let method_name_str = IdentId::get_name(method_name);
        let func_id = self.func.add_attr_writer(method_name_str, ivar_name);
        self.add_method(class_id, method_name, func_id);
        interp.class_version_inc();
        method_name
    }

    pub(crate) fn compile_script(
        &mut self,
        code: String,
        path: impl Into<PathBuf>,
    ) -> Result<FuncId> {
        match Parser::parse_program(code, path.into()) {
            Ok(res) => self.func.compile_script(res.node, res.source_info),
            Err(err) => Err(MonorubyErr::parse(err)),
        }
    }

    pub fn compile_script_with_binding(
        &mut self,
        code: String,
        path: impl Into<PathBuf>,
        context: Option<LvarCollector>,
    ) -> Result<(FuncId, LvarCollector)> {
        match Parser::parse_program_binding(code, path.into(), context, None) {
            Ok(res) => {
                let collector = res.lvar_collector;
                let fid = self.func.compile_script(res.node, res.source_info)?;
                Ok((fid, collector))
            }
            Err(err) => Err(MonorubyErr::parse(err)),
        }
    }
}

impl Globals {
    #[cfg(feature = "emit-bc")]
    pub(crate) fn dump_bc(&self) {
        self.func
            .functions()
            .iter()
            .skip(1)
            .for_each(|info| match &info.kind {
                FuncKind::ISeq(_) => info.dump_bc(self),
                _ => {}
            });
    }
}
