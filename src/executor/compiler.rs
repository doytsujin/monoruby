#[cfg(any(feature = "emit-asm", feature = "log-jit"))]
use std::time::Instant;

use monoasm::*;
use monoasm_macro::monoasm;
use paste::paste;

use crate::executor::compiler::vmgen::get_func_data;

use super::*;

mod jitgen;
mod vmgen;

pub type EntryPoint = extern "C" fn(&mut Interp, &mut Globals, FuncId) -> Option<Value>;

pub type Invoker =
    extern "C" fn(&mut Interp, &mut Globals, FuncId, Value, *const Value, usize) -> Option<Value>;

///
/// Bytecode compiler
///
/// This generates x86-64 machine code from a bytecode.
///
pub struct Codegen {
    pub jit: JitMemory,
    pub class_version: DestLabel,
    pub const_version: DestLabel,
    pub entry_panic: DestLabel,
    pub vm_entry: CodePtr,
    pub vm_entry_point: EntryPoint,
    entry_find_method: DestLabel,
    pub vm_return: DestLabel,
    pub dispatch: Vec<CodePtr>,
    pub invoker: Invoker,
    pub func_data: FuncDataLabels,
}

pub struct FuncDataLabels {
    pub func_offset: DestLabel,
    pub func_address: DestLabel,
    pub func_pc: DestLabel,
    pub func_ret: DestLabel,
}

fn conv(reg: u16) -> i64 {
    reg as i64 * 8 + 16
}

//
// Runtime functions.
//

///
/// Get an absolute address of the given method.
///
/// If no method was found, return None (==0u64).
///
pub extern "C" fn get_func_address(
    interp: &mut Interp,
    globals: &mut Globals,
    func_name: IdentId,
    args_len: usize,
    receiver: Value,
    funcid_patch: &mut FuncId,
) -> Option<CodePtr> {
    let func_id = globals.get_method(receiver.class_id(), func_name, args_len)?;
    *funcid_patch = func_id;
    Some(interp.codegen.compile_on_demand(globals, func_id))
}

extern "C" fn define_method(
    _interp: &mut Interp,
    globals: &mut Globals,
    name: IdentId,
    func: FuncId,
) {
    globals.class.add_method(OBJECT_CLASS, name, func);
}

pub extern "C" fn unimplemented_inst(_: &mut Interp, _: &mut Globals) {
    panic!("unimplemented inst.");
}

pub extern "C" fn panic(_: &mut Interp, _: &mut Globals) {
    panic!("panic in jit code.");
}

extern "C" fn get_error_location(
    _interp: &mut Interp,
    globals: &mut Globals,
    func_id: FuncId,
    pc: BcPc,
) {
    let normal_info = globals.func[func_id].as_normal();
    let sourceinfo = normal_info.sourceinfo.clone();
    let bc_base = globals.func[func_id].inst_pc();
    let loc = normal_info.sourcemap[pc - bc_base];
    globals.push_error_location(loc, sourceinfo);
}

macro_rules! cmp_main {
    ($op:ident) => {
        paste! {
            fn [<cmp_ $op>](&mut self) {
                let exit = self.jit.label();
                let generic = self.jit.label();
                self.guard_rdi_fixnum(generic);
                self.guard_rsi_fixnum(generic);
                monoasm! { self.jit,
                    xorq rax, rax;
                    cmpq rdi, rsi;
                    [<set $op>] rax;
                    shlq rax, 3;
                    orq rax, (FALSE_VALUE);
                exit:
                };
                self.jit.select(1);
                monoasm!(self.jit,
                generic:
                    // generic path
                    movq rax, ([<cmp_ $op _values>]);
                    call rax;
                    jmp  exit;
                );
                self.jit.select(0);
            }
        }
    };
    ($op1:ident, $($op2:ident),+) => {
        cmp_main!($op1);
        cmp_main!($($op2),+);
    };
}

macro_rules! cmp_ri_main {
    ($op:ident) => {
        paste! {
            fn [<cmp_ri_ $op>](&mut self) {
                let exit = self.jit.label();
                let generic = self.jit.label();
                self.guard_rdi_fixnum(generic);
                monoasm! { self.jit,
                    xorq rax, rax;
                    cmpq rdi, rsi;
                    [<set $op>] rax;
                    shlq rax, 3;
                    orq rax, (FALSE_VALUE);
                exit:
                };

                self.jit.select(1);
                monoasm!(self.jit,
                generic:
                    // generic path
                    movq rax, ([<cmp_ $op _values>]);
                    call rax;
                    jmp  exit;
                );
                self.jit.select(0);
            }
        }
    };
    ($op1:ident, $($op2:ident),+) => {
        cmp_ri_main!($op1);
        cmp_ri_main!($($op2),+);
    };
}

impl Codegen {
    pub fn new() -> Self {
        let mut jit = JitMemory::new();
        jit.add_page();
        let class_version = jit.const_i64(0);
        let const_version = jit.const_i64(0);
        let entry_panic = jit.label();
        let entry_find_method = jit.label();
        let jit_return = jit.label();
        let vm_return = jit.label();
        let func_offset = jit.const_i64(0);
        let func_address = jit.const_i64(0);
        let func_pc = jit.const_i64(0);
        let func_ret = jit.const_i64(0);
        monoasm!(&mut jit,
        entry_panic:
            movq rdi, rbx;
            movq rsi, r12;
            movq rax, (panic);
            jmp rax;
        entry_find_method:
            movq rdi, rbx;
            movq rsi, r12;
            movq rax, (get_func_address);
            jmp  rax;
        vm_return:
            // check call_kind.
            movl r15, [rbp - 8];
            testq r15, r15;
            jne  jit_return;
            // save return value
            movq r15, rax;
            movq rdi, rbx;
            movq rsi, r12;
            movl rdx, [rbp - 0x4];
            movq rcx, r13;
            subq rcx, 8;
            movq rax, (get_error_location);
            call rax;
            // restore return value
            movq rax, r15;
        jit_return:
            leave;
            ret;
        );
        // method invoker.
        let invoker: extern "C" fn(
            &mut Interp,
            &mut Globals,
            FuncId,
            Value,
            *const Value,
            usize,
        ) -> Option<Value> = unsafe { std::mem::transmute(jit.get_current_address().as_ptr()) };
        let loop_exit = jit.label();
        let loop_ = jit.label();
        // rdi: &mut Interp
        // rsi: &mut Globals
        // rdx: FuncId
        // rcx: receiver: Value
        // r8:  *args: *const Value
        // r9:  len: usize
        monoasm! { &mut jit,
            pushq rbp;
            movq rbp, rsp;
            pushq rbx;
            pushq r12;
            pushq r13;
            pushq r15;
            movq rbx, rdi;
            movq r12, rsi;
            pushq r9;
            pushq r8;
            // set meta/call_kind = 0(VM)
            // set meta/func_id
            pushq rdx;
            // set self (= receiver)
            pushq rcx;
            lea rcx, [rip + func_offset]; // rcx: &mut FuncData
            movq rax, (get_func_data);
            call rax;

            movq r13, [rip + func_pc];    // r13: BcPc
            addq rsp, 16;
            // rcx <- *args
            popq rcx;
            // r8 <- len
            popq r8;
            //
            //       +-------------+
            // +0x08 |             |
            //       +-------------+
            //  0x00 |             | <- rsp
            //       +-------------+
            // -0x08 | return addr | len
            //       +-------------+
            // -0x10 |   old rbp   | args
            //       +-------------+
            // -0x18 |    meta     | func_id
            //       +-------------+
            // -0x20 |     %0      | receiver
            //       +-------------+
            // -0x28 | %1(1st arg) |
            //       +-------------+
            //       |             |
            //
            movq rdi, r8;
            testq r8, r8;
            jeq  loop_exit;
            negq r8;
        loop_:
            movq rax, [rcx + r8 * 8 + 8];
            movq [rsp + r8 * 8- 0x20], rax;
            addq r8, 1;
            jne  loop_;
        loop_exit:

            movq rax, [rip + func_address];
            call rax;
            popq r15;
            popq r13;
            popq r12;
            popq rbx;
            leave;
            ret;
        };

        // dispatch table.
        let entry_unimpl = jit.get_current_address();
        monoasm! { jit,
                movq rdi, rbx;
                movq rsi, r12;
                movq rax, (super::compiler::unimplemented_inst);
                call rax;
                leave;
                ret;
        };
        let dispatch = vec![entry_unimpl; 256];
        let func_data = FuncDataLabels {
            func_offset,
            func_address,
            func_pc,
            func_ret,
        };
        let mut codegen = Self {
            jit,
            class_version,
            const_version,
            entry_panic,
            entry_find_method,
            vm_entry: entry_unimpl,
            vm_entry_point: unsafe { std::mem::transmute(entry_unimpl.as_ptr()) },
            vm_return,
            dispatch,
            invoker,
            func_data,
        };
        codegen.vm_entry_point = codegen.construct_vm();
        codegen
    }

    fn guard_rdi_rsi_fixnum(&mut self, generic: DestLabel) {
        self.guard_rdi_fixnum(generic);
        self.guard_rsi_fixnum(generic);
    }

    fn guard_rdi_fixnum(&mut self, generic: DestLabel) {
        monoasm!(self.jit,
            // check whether lhs is fixnum.
            testq rdi, 0x1;
            jeq generic;
        );
    }

    fn guard_rsi_fixnum(&mut self, generic: DestLabel) {
        monoasm!(self.jit,
            // check whether rhs is fixnum.
            testq rsi, 0x1;
            jeq generic;
        );
    }

    cmp_main!(eq, ne, lt, le, gt, ge);
    cmp_ri_main!(eq, ne, lt, le, gt, ge);

    fn call_unop(&mut self, func: u64, entry_return: DestLabel) {
        monoasm!(self.jit,
            movq rdx, rdi;
            movq rdi, rbx;
            movq rsi, r12;
            movq rax, (func);
            call rax;
            testq rax, rax;
            jeq entry_return;
        );
    }

    fn call_binop(&mut self, func: u64, entry_return: DestLabel) {
        monoasm!(self.jit,
            movq rdx, rdi;
            movq rcx, rsi;
            movq rdi, rbx;
            movq rsi, r12;
            movq rax, (func);
            call rax;
            testq rax, rax;
            jeq entry_return;
        );
    }

    ///
    /// ## stack layout for JIT-ed code (just after prologue).
    ///
    ///~~~text
    ///       +-------------+
    /// +0x08 | return addr |
    ///       +-------------+
    ///  0x00 |  prev rbp   | <- rbp
    ///       +-------------+       +-------+-------+-------+-------+
    /// -0x08 |    meta     |  meta | 0:VM 1:JIT 2: |    FuncId     |
    ///       +-------------+       +-------+-------+-------+-------+
    /// -0x10 |     %0      |             48      32      16       0
    ///       +-------------+
    /// -0x18 |     %1      |
    ///       +-------------+
    ///       |      :      |
    ///       +-------------+
    /// -0xy0 |    %(n-1)   | <- rsp
    ///       +-------------+
    ///       |      :      |
    /// ~~~

    /// ## ABI of JIT-compiled code.
    ///
    /// ### argument registers:
    ///  - rdi: number pf args
    ///
    /// ### global registers:
    ///  - rbx: &mut Interp
    ///  - r12: &mut Globals
    ///  - r13: pc (dummy for JIT-ed code)
    ///
    /// ## stack layout when just after the code is called
    /// ~~~text
    ///       +-------------+
    /// -0x00 | return addr | <- rsp
    ///       +-------------+
    /// -0x08 |  (old rbp)  |
    ///       +-------------+
    /// -0x10 |    meta     |
    ///       +-------------+
    /// -0x18 |     %0      |
    ///       +-------------+
    /// -0x20 | %1(1st arg) |
    ///       +-------------+
    ///       |             |
    /// ~~~~
    ///
    ///  - meta and arguments is set by caller.
    ///  - (old rbp) is to be set by callee.
    ///
    pub fn exec_toplevel(&mut self, globals: &mut Globals) -> EntryPoint {
        let main_id = globals.get_main_func();
        let main = self.compile_on_demand(globals, main_id);
        let entry = self.jit.label();
        //       +-------------+
        // -0x00 |             | <- rsp
        //       +-------------+
        // -0x08 | return addr |
        //       +-------------+
        // -0x10 |  (old rbp)  |
        //       +-------------+
        // -0x18 |    meta     |
        //       +-------------+
        // -0x20 |     %0      |
        //       +-------------+
        monoasm!(self.jit,
        entry:
            pushq rbp;
            pushq rbx;
            pushq r12;
            movq rbx, rdi;
            movq r12, rsi;
            movl [rsp - 0x14], rdx;
            movl [rsp - 0x18], 1;
            movq [rsp - 0x20], (NIL_VALUE);
            xorq rdi, rdi;
            movq rax, (main.as_ptr());
            call rax;
            popq r12;
            popq rbx;
            popq rbp;
            ret;
        );
        self.jit.finalize();
        unsafe { std::mem::transmute(self.jit.get_label_address(entry).as_ptr()) }
    }

    pub fn compile_on_demand(&mut self, globals: &mut Globals, func_id: FuncId) -> CodePtr {
        match globals.func[func_id].jit_label() {
            Some(dest) => dest,
            None => {
                let mut info = std::mem::take(&mut globals.func[func_id]);
                let label = self.jit_compile(&mut info, &globals.func);
                globals.func[func_id] = info;
                label
            }
        }
    }

    fn jit_compile(&mut self, func: &mut FuncInfo, store: &FnStore) -> CodePtr {
        #[cfg(any(feature = "emit-asm", feature = "log-jit"))]
        let now = Instant::now();
        let label = match &func.kind {
            FuncKind::Normal(info) => self.jit_compile_normal(info, store),
            FuncKind::Builtin { abs_address } => self.wrap_builtin(*abs_address),
        };
        func.set_jit_label(label);
        self.jit.finalize();
        #[cfg(any(feature = "emit-asm", feature = "log-jit"))]
        {
            eprintln!("jit compile: {:?}", func.id());
            #[cfg(any(feature = "emit-asm"))]
            {
                let (start, code_end, end) = self.jit.code_block.last().unwrap();
                eprintln!(
                    "offset:{:?} code: {} bytes  data: {} bytes",
                    start,
                    *code_end - *start,
                    *end - *code_end
                );
                eprintln!("{}", self.jit.dump_code().unwrap());
            }
            eprintln!("jit compile elapsed:{:?}", now.elapsed());
        }
        label
    }

    pub fn wrap_builtin(&mut self, abs_address: u64) -> CodePtr {
        //
        // generate a wrapper for a builtin function which has C ABI.
        // stack layout at the point of just after a wrapper was called.
        //
        //       +-------------+
        //  0x00 | return addr | <- rsp
        //       +-------------+
        // -0x08 |             |
        //       +-------------+
        // -0x10 |    meta     |
        //       +-------------+
        // -0x18 |  %0 (self)  |
        //       +-------------+
        // -0x20 | %1(1st arg) |
        //       +-------------+
        //
        // argument registers:
        //   rdi: number of args
        //
        // global registers:
        //   rbx: &mut Interp
        //   r12: &mut Globals
        //   r13: pc (dummy for builtin funcions)
        //
        //   stack_offset: [rip + func_offset] (not used, because we can hard-code it.)
        //
        let label = self.jit.get_current_address();
        // calculate stack offset
        monoasm!(self.jit,
            movq rcx, rdi;
            movq rax, rdi;
            andq rax, (0b01);
            addq rax, rdi;
            shlq rax, 3;
            addq rax, 16;
        );
        monoasm!(self.jit,
            lea  rdx, [rsp - 0x20];
            pushq rbp;
            movq rdi, rbx;
            movq rsi, r12;
            //movq rcx, (arity);
            movq rbp, rsp;
            subq rsp, rax;
            movq rax, (abs_address);
            // fn(&mut Interp, &mut Globals, *const Value, len:usize)
            call rax;
            leave;
            ret;
        );
        label
    }
}

impl Codegen {
    pub fn precompile(&mut self, store: &mut FnStore) {
        for func in store.funcs_mut().iter_mut() {
            match &func.kind {
                FuncKind::Normal(_) => {
                    func.set_jit_label(self.vm_entry);
                }
                FuncKind::Builtin { abs_address } => {
                    let label = self.wrap_builtin(*abs_address);
                    func.set_jit_label(label);
                }
            };
        }
    }
}
