use std::time::Instant;

use monoasm::*;
use monoasm_macro::monoasm;

use super::*;

pub type JitFunc<'r, 's> = extern "C" fn(&'r mut Interp, &'s mut Globals) -> Option<Value>;

///
/// Bytecode compiler
///
/// This generates x86-64 machine code from a bytecode.
///
pub struct JitGen {
    pub jit: JitMemory,
    class_version: DestLabel,
    pub entry_panic: DestLabel,
    entry_find_method: DestLabel,
    pub entry_return: DestLabel,
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
extern "C" fn get_func_absolute_address(
    interp: &mut Interp,
    globals: &mut Globals,
    func_name: IdentId,
    args_len: usize,
) -> Option<CodePtr> {
    let func_id = interp.get_method(globals, func_name, args_len)?;
    match globals.func[func_id].jit_label() {
        Some(dest) => Some(dest),
        None => {
            let mut info = std::mem::take(&mut globals.func[func_id]);
            let label = interp.jit_gen.jit_compile(&mut info, &globals.func);
            globals.func[func_id] = info;
            Some(label)
        }
    }
}

extern "C" fn define_method(
    _interp: &mut Interp,
    globals: &mut Globals,
    func_name: IdentId,
    func_id: FuncId,
) {
    globals.class.add_method(0, func_name, func_id);
}

pub extern "C" fn unimplemented_inst(_: &mut Interp, _: &mut Globals) {
    panic!("unimplemented inst.");
}

pub extern "C" fn panic(_: &mut Interp, _: &mut Globals) {
    panic!("panic in jit code.");
}

impl JitGen {
    pub fn new() -> Self {
        let mut jit = JitMemory::new();
        let class_version = jit.const_i64(0);
        let entry_panic = jit.label();
        let entry_find_method = jit.label();
        let entry_return = jit.label();
        monoasm!(&mut jit,
        entry_panic:
            movq rdi, rbx;
            movq rsi, r12;
            movq rax, (panic);
            jmp rax;
        entry_find_method:
            movq rdi, rbx;
            movq rsi, r12;
            movq rax, (get_func_absolute_address);
            jmp rax;
        entry_return:
            leave;
            ret;
        );
        Self {
            jit,
            class_version,
            entry_panic,
            entry_find_method,
            entry_return,
        }
    }

    fn prologue(&mut self, regs: usize) {
        monoasm!(self.jit,
            pushq rbp;
            movq rbp, rsp;
            subq rsp, ((regs + regs % 2) * 8);
        );
    }

    fn epilogue(&mut self) {
        monoasm!(self.jit,
            leave;
            ret;
        );
    }

    fn load_binary_args(&mut self, lhs: u16, rhs: u16) {
        monoasm!(self.jit,
            movq rdi, [rbp - (conv(lhs))];
            movq rsi, [rbp - (conv(rhs))];
        );
    }

    pub(super) fn guard_rdi_rsi_fixnum(&mut self, generic: DestLabel) {
        self.guard_rdi_fixnum(generic);
        self.guard_rsi_fixnum(generic);
    }

    pub(super) fn guard_rdi_fixnum(&mut self, generic: DestLabel) {
        monoasm!(self.jit,
            // check whether lhs is fixnum.
            testq rdi, 0x1;
            jeq generic;
        );
    }

    pub(super) fn guard_rsi_fixnum(&mut self, generic: DestLabel) {
        monoasm!(self.jit,
            // check whether rhs is fixnum.
            testq rsi, 0x1;
            jeq generic;
        );
    }

    fn generic_op(&mut self, ret: u16, func: u64) {
        monoasm!(self.jit,
            // generic path
            movq rax, (func);
            call rax;
            // store the result to return reg.
            movq [rbp - (conv(ret))], rax;
        );
    }

    fn generic(&mut self, generic: DestLabel, exit: DestLabel, ret: u16, func: u64) {
        monoasm!(self.jit,
        generic:
            // generic path
            movq rax, (func);
            call rax;
            // store the result to return reg.
            movq [rbp - (conv(ret))], rax;
        exit:
        );
    }

    fn fast_add(&mut self, exit: DestLabel, generic: DestLabel, ret: u16) {
        monoasm!(self.jit,
            // fastpath
            movq rax, rdi;
            subq rax, 1;
            addq rax, rsi;
            jo generic;
            // store the result to return reg.
            movq [rbp - (conv(ret))], rax;
            jmp exit;
        );
    }

    fn fast_sub(&mut self, exit: DestLabel, generic: DestLabel, ret: u16) {
        monoasm!(self.jit,
            // fastpath
            movq rax, rdi;
            subq rax, rsi;
            jo generic;
            addq rax, 1;
            // store the result to return reg.
            movq [rbp - (conv(ret))], rax;
            jmp exit;
        );
    }

    fn generic_bit_or(&mut self, generic: DestLabel, exit: DestLabel, ret: u16) {
        monoasm!(self.jit,
            // fastpath
            orq rdi, rsi;
            // store the result to return reg.
            movq [rbp - (conv(ret))], rdi;
            jmp exit;
        );
        self.generic(generic, exit, ret, bitor_values as _);
    }

    fn generic_bit_and(&mut self, generic: DestLabel, exit: DestLabel, ret: u16) {
        monoasm!(self.jit,
            // fastpath
            andq rdi, rsi;
            // store the result to return reg.
            movq [rbp - (conv(ret))], rdi;
            jmp exit;
        );
        self.generic(generic, exit, ret, bitand_values as _);
    }

    fn generic_bit_xor(&mut self, generic: DestLabel, exit: DestLabel, ret: u16) {
        monoasm!(self.jit,
            // fastpath
            xorq rdi, rsi;
            addq rdi, 1;
            // store the result to return reg.
            movq [rbp - (conv(ret))], rdi;
            jmp exit;
        );
        self.generic(generic, exit, ret, bitxor_values as _);
    }

    fn shift_under(&mut self, under: DestLabel, after: DestLabel) {
        let zero = self.jit.label();
        monoasm!(self.jit,
        under:
            testq rdi, rdi;
            jns zero;
            xorq rdi, rdi;
            subq rdi, 1;
            jmp after;
        zero:
            xorq rdi, rdi;
            jmp after;
        );
    }

    fn generic_shr(&mut self, generic: DestLabel, exit: DestLabel, ret: u16) {
        let shl = self.jit.label();
        let after = self.jit.label();
        let under = self.jit.label();
        monoasm!(self.jit,
            // fastpath
            movq rcx, rsi;
            sarq rcx, 1;
            js shl;
            cmpq rcx, 64;
            jge under;
            sarq rdi, rcx;
        after:
            orq rdi, 1;
            // store the result to return reg.
            movq [rbp - (conv(ret))], rdi;
            jmp exit;
        shl:
            negq rcx;
            lzcntq rax, rdi;
            cmpq rcx, rax;
            jgt generic;
            subq rdi, 1;
            salq rdi, rcx;
            jmp after;
        );
        self.shift_under(under, after);
        self.generic(generic, exit, ret, shr_values as _);
    }

    fn generic_shl(&mut self, generic: DestLabel, exit: DestLabel, ret: u16) {
        let shr = self.jit.label();
        let after = self.jit.label();
        let under = self.jit.label();
        monoasm!(self.jit,
            // fastpath
            movq rcx, rsi;
            sarq rcx, 1;
            js shr;
            lzcntq rax, rdi;
            cmpq rcx, rax;
            jgt generic;
            subq rdi, 1;
            salq rdi, rcx;
        after:
            orq rdi, 1;
            // store the result to return reg.
            movq [rbp - (conv(ret))], rdi;
            jmp exit;
        shr:
            negq rcx;
            cmpq rcx, 64;
            jge under;
            sarq rdi, rcx;
            jmp after;
        );
        self.shift_under(under, after);
        self.generic(generic, exit, ret, shl_values as _);
    }

    ///
    /// ## stack layout for JIT-ed code (jut after prologue).
    ///
    ///~~~text
    ///       +-------------+
    /// +0x08 | return addr |
    ///       +-------------+
    ///  0x00 |  prev rbp   | <- rbp
    ///       +-------------+
    /// -0x08 |    meta     |
    ///       +-------------+
    /// -0x10 |     %0      |
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
    /// ### global registers:
    ///  - rbx: &mut Interp
    ///  - r12: &mut Globals
    ///  - r13: pc (dummy for JIT-ed code)
    ///
    /// ### stack_offset:
    ///
    ///  - \[rip + func_offset\]: (not used, hard-coded.)
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
    pub fn exec_toplevel(&mut self, globals: &mut Globals) -> JitFunc {
        let main_id = globals.get_main_func();
        let mut info = std::mem::take(&mut globals.func[main_id]);
        let main = self.jit_compile(&mut info, &globals.func);
        globals.func[main_id] = info;
        let entry = self.jit.label();
        monoasm!(self.jit,
        entry:
            pushq rbp;
            pushq rbx;
            pushq r12;
            movq rbx, rdi;
            movq r12, rsi;
            movq [rsp - 0x20], (NIL_VALUE);
            movq rax, (main.as_ptr());
            call rax;
            popq r12;
            popq rbx;
            popq rbp;
            ret;
        );
        self.jit.finalize();
        self.jit.get_label_addr2(entry)
    }

    fn jit_compile(&mut self, func: &mut FuncInfo, store: &FnStore) -> CodePtr {
        let now = Instant::now();
        let label = match &func.kind {
            FuncKind::Normal(info) => self.jit_compile_normal(info, store),
            FuncKind::Builtin { abs_address } => {
                self.jit_compile_builtin(*abs_address, func.arity())
            }
        };
        func.set_jit_label(label);
        self.jit.finalize();
        #[cfg(feature = "emit-asm")]
        {
            eprintln!("jit compile: {:?}", func.id());
            let (start, code_end, end) = self.jit.code_block.last().unwrap();
            eprintln!(
                "offset:{:?} code: {} bytes  data: {} bytes",
                start,
                *code_end - *start,
                *end - *code_end
            );
            eprintln!("{}", self.jit.dump_code().unwrap());
            eprintln!("jit compile elapsed:{:?}", now.elapsed());
        }
        label
    }

    pub fn jit_compile_builtin(&mut self, abs_address: u64, arity: usize) -> CodePtr {
        //
        // generate a wrapper for a builtin function which has C ABI.
        // stack layout at the point of just after execution of call instruction.
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
        //
        // global registers:
        //   rbx: &mut Interp
        //   r12: &mut Globals
        //   r13: pc (dummy for builtin funcions)
        //
        //   stack_offset: [rip + func_offset] (not used, because we can hard-code it.)
        //
        let label = self.jit.get_current_address();
        let offset = (arity + arity % 2) * 8 + 16;
        monoasm!(self.jit,
            pushq rbp;
            movq rdi, rbx;
            movq rsi, r12;
            lea  rdx, [rsp - 0x18];
            movq rcx, (arity);
            movq rax, (abs_address);
            movq rbp, rsp;
            subq rsp, (offset);
            // fn(&mut Interp, &mut Globals, *const Value, len:usize)
            call rax;
            leave;
            ret;
        );
        label
    }

    fn jit_compile_normal(&mut self, func: &NormalFuncInfo, store: &FnStore) -> CodePtr {
        macro_rules! cmp {
            ($lhs:ident, $rhs:ident, $ret:ident, $cmp:ident, $generic:ident) => {{
                let generic = self.jit.label();
                let exit = self.jit.label();
                self.load_binary_args($lhs, $rhs);
                self.guard_rdi_fixnum(generic);
                self.guard_rsi_fixnum(generic);
                monoasm!(self.jit,
                    // fastpath
                    xorq rax,rax;
                    cmpq rdi, rsi;
                    $cmp rax;
                    shlq rax, 3;
                    orq rax, (FALSE_VALUE);
                    jmp exit;
                generic:
                    // generic path
                    movq rax, ($generic);
                    call rax;
                exit:
                    // store the result to return reg.
                    movq [rbp - (conv($ret))], rax;
                );
            }};
        }

        macro_rules! cmp_ri {
            ($lhs:ident, $rhs:ident, $ret:ident, $set:ident, $generic_func:ident) => {{
                let generic = self.jit.label();
                let exit = self.jit.label();
                monoasm!(self.jit,
                    movq rdi, [rbp - (conv($lhs))];
                    movq rsi, (Value::integer($rhs as i64).get());
                );
                self.guard_rdi_fixnum(generic);
                monoasm!(self.jit,
                    xorq rax, rax;
                    cmpq rdi, rsi;
                    $set rax;
                    shlq rax, 3;
                    orq rax, (FALSE_VALUE);
                    jmp exit;
                generic:
                    movq rax, ($generic_func);
                    call rax;
                exit:
                    movq [rbp - (conv($ret))], rax;
                );
            }};
        }

        let label = self.jit.get_current_address();
        let mut labels = vec![];
        for _ in func.bytecode() {
            labels.push(self.jit.label());
        }
        self.prologue(func.total_reg_num());
        for (idx, op) in func.bytecode().iter().enumerate() {
            self.jit.bind_label(labels[idx]);
            match BcOp::from_u64(*op) {
                BcOp::Integer(ret, i) => {
                    let i = Value::int32(i).get();
                    monoasm!(self.jit,
                      movq [rbp - (conv(ret))], (i);
                    );
                }
                BcOp::Const(ret, id) => {
                    let v = store.get_literal(id);
                    //if v.is_packed_value() {
                    monoasm!(self.jit,
                      movq rax, (v.get());
                      movq [rbp - (conv(ret))], rax;
                    );
                    /*} else {
                        monoasm!(self.jit,
                          movq rdi, (v.get());
                          movq rax, (Value::dup);
                          call rax;
                          movq [rbp - (conv(ret))], rax;
                        );
                    }*/
                }
                BcOp::Nil(ret) => {
                    monoasm!(self.jit,
                      movq rax, (NIL_VALUE);
                      movq [rbp - (conv(ret))], rax;
                    );
                }
                BcOp::Neg(dst, src) => {
                    monoasm!(self.jit,
                      movq rdi, [rbp - (conv(src))];
                      movq rax, (neg_value);
                      call rax;
                      movq [rbp - (conv(dst))], rax;
                    );
                }
                BcOp::Add(ret, lhs, rhs) => {
                    let generic = self.jit.label();
                    let exit = self.jit.label();
                    self.load_binary_args(lhs, rhs);
                    self.guard_rdi_rsi_fixnum(generic);
                    self.fast_add(exit, generic, ret);
                    self.generic(generic, exit, ret, add_values as _);
                }
                BcOp::Addri(ret, lhs, rhs) => {
                    let generic = self.jit.label();
                    let exit = self.jit.label();
                    let rhs = Value::int32(rhs as i32).get();
                    monoasm!(self.jit,
                        movq rdi, [rbp - (conv(lhs))];
                        movq rsi, (rhs);
                    );
                    self.guard_rdi_fixnum(generic);
                    self.fast_add(exit, generic, ret);
                    self.generic(generic, exit, ret, add_values as _);
                }
                BcOp::Sub(ret, lhs, rhs) => {
                    let generic = self.jit.label();
                    let exit = self.jit.label();
                    self.load_binary_args(lhs, rhs);
                    self.guard_rdi_rsi_fixnum(generic);
                    self.fast_sub(exit, generic, ret);
                    self.generic(generic, exit, ret, sub_values as _);
                }
                BcOp::Subri(ret, lhs, rhs) => {
                    let generic = self.jit.label();
                    let exit = self.jit.label();
                    let rhs = Value::int32(rhs as i32).get();
                    monoasm!(self.jit,
                        movq rdi, [rbp - (conv(lhs))];
                        movq rsi, (rhs);
                    );
                    self.guard_rdi_fixnum(generic);
                    self.fast_sub(exit, generic, ret);
                    self.generic(generic, exit, ret, sub_values as _);
                }

                BcOp::Mul(ret, lhs, rhs) => {
                    self.load_binary_args(lhs, rhs);
                    self.generic_op(ret, mul_values as _);
                }
                BcOp::Div(ret, lhs, rhs) => {
                    self.load_binary_args(lhs, rhs);
                    self.generic_op(ret, div_values as _);
                }
                BcOp::BitOr(ret, lhs, rhs) => {
                    let generic = self.jit.label();
                    let exit = self.jit.label();
                    self.load_binary_args(lhs, rhs);
                    self.guard_rdi_rsi_fixnum(generic);
                    self.generic_bit_or(generic, exit, ret);
                }
                BcOp::BitAnd(ret, lhs, rhs) => {
                    let generic = self.jit.label();
                    let exit = self.jit.label();
                    self.load_binary_args(lhs, rhs);
                    self.guard_rdi_rsi_fixnum(generic);
                    self.generic_bit_and(generic, exit, ret);
                }
                BcOp::BitXor(ret, lhs, rhs) => {
                    let generic = self.jit.label();
                    let exit = self.jit.label();
                    self.load_binary_args(lhs, rhs);
                    self.guard_rdi_rsi_fixnum(generic);
                    self.generic_bit_xor(generic, exit, ret);
                }
                BcOp::Shr(ret, lhs, rhs) => {
                    let generic = self.jit.label();
                    let exit = self.jit.label();
                    self.load_binary_args(lhs, rhs);
                    self.guard_rdi_rsi_fixnum(generic);
                    self.generic_shr(generic, exit, ret);
                }
                BcOp::Shl(ret, lhs, rhs) => {
                    let generic = self.jit.label();
                    let exit = self.jit.label();
                    self.load_binary_args(lhs, rhs);
                    self.guard_rdi_rsi_fixnum(generic);
                    self.generic_shl(generic, exit, ret);
                }
                BcOp::Eq(ret, lhs, rhs) => cmp!(lhs, rhs, ret, seteq, cmp_eq_values),
                BcOp::Ne(ret, lhs, rhs) => cmp!(lhs, rhs, ret, setne, cmp_ne_values),
                BcOp::Ge(ret, lhs, rhs) => cmp!(lhs, rhs, ret, setge, cmp_ge_values),
                BcOp::Gt(ret, lhs, rhs) => cmp!(lhs, rhs, ret, setgt, cmp_gt_values),
                BcOp::Le(ret, lhs, rhs) => cmp!(lhs, rhs, ret, setle, cmp_le_values),
                BcOp::Lt(ret, lhs, rhs) => cmp!(lhs, rhs, ret, setlt, cmp_lt_values),
                BcOp::Eqri(ret, lhs, rhs) => cmp_ri!(lhs, rhs, ret, seteq, cmp_eq_values),
                BcOp::Neri(ret, lhs, rhs) => cmp_ri!(lhs, rhs, ret, setne, cmp_ne_values),
                BcOp::Geri(ret, lhs, rhs) => cmp_ri!(lhs, rhs, ret, setge, cmp_ge_values),
                BcOp::Gtri(ret, lhs, rhs) => cmp_ri!(lhs, rhs, ret, setgt, cmp_gt_values),
                BcOp::Leri(ret, lhs, rhs) => cmp_ri!(lhs, rhs, ret, setle, cmp_le_values),
                BcOp::Ltri(ret, lhs, rhs) => cmp_ri!(lhs, rhs, ret, setlt, cmp_lt_values),
                BcOp::Mov(dst, src) => {
                    monoasm!(self.jit,
                      movq rax, [rbp - (conv(src))];
                      movq [rbp - (conv(dst))], rax;
                    );
                }
                BcOp::Ret(lhs) => {
                    monoasm!(self.jit,
                        movq rax, [rbp - (conv(lhs))];
                    );
                    self.epilogue();
                }
                BcOp::FnCall(ret, id) => {
                    // set arguments to a callee stack.
                    //
                    //       +-------------+
                    //  0x00 |             | <- rsp
                    //       +-------------+
                    // -0x08 | return addr |
                    //       +-------------+
                    // -0x10 |   old rbp   |
                    //       +-------------+
                    // -0x18 |    meta     |
                    //       +-------------+
                    // -0x20 |     %0      |
                    //       +-------------+
                    // -0x28 | %1(1st arg) |
                    //       +-------------+
                    //       |             |
                    //
                    let CallsiteInfo {
                        name, args, len, ..
                    } = store[id];
                    let sp_max = 0x40 + (len as u64 + (len % 2) as u64) * 8;
                    // set self
                    monoasm!(self.jit,
                        movq rax, [rbp - 16];
                        movq [rsp - 0x20], rax;
                    );
                    // set arguments
                    for i in 0..len {
                        let reg = args + i;
                        monoasm!(self.jit,
                            movq rax, [rbp - (conv(reg))];
                            movq [rsp - ((0x28 + i * 8) as i64)], rax;
                        );
                    }
                    let l1 = self.jit.label();
                    let exit = self.jit.label();
                    let saved_class_version = self.jit.const_i64(-1);
                    let class_version = self.class_version;
                    let entry_find_method = self.entry_find_method;
                    let entry_panic = self.entry_panic;
                    let entry_return = self.entry_return;
                    monoasm!(self.jit,
                        movq rax, [rip + class_version];
                        cmpq [rip + saved_class_version], rax;
                        jeq l1;
                        // call site stub code.
                        // push down sp to avoid destroying arguments area.
                        subq rsp, (sp_max);
                        movq rdx, (u32::from(name)); // IdentId
                        movq rcx, (len as usize); // args_len: usize
                        call entry_find_method;
                        // absolute address was returned to rax.
                        addq rsp, (sp_max);
                        testq rax, rax;
                        jeq entry_return;
                        lea rdi, [rip + exit];
                        // calculate a displacement to the function address.
                        subq rax, rdi;
                        // set patch point address (= return address - 4) to rdi.
                        subq rdi, 4;
                        // apply patch.
                        movl [rdi], rax;
                        movq rax, [rip + class_version];
                        movq [rip + saved_class_version], rax;
                    l1:
                        // patch point
                        call entry_panic;
                    exit:
                        testq rax, rax;
                        jeq entry_return;
                    );
                    if ret != 0 {
                        monoasm!(self.jit,
                            movq [rbp - (conv(ret))], rax;
                        );
                    }
                }
                BcOp::MethodDef(id) => {
                    let MethodDefInfo { name, func } = store[id];
                    let class_version = self.class_version;
                    monoasm!(self.jit,
                        addq [rip + class_version], 1;
                        movq rdi, rbx; // &mut Interp
                        movq rsi, r12; // &Globals
                        movq rdx, (u32::from(name)); // IdentId
                        movq rcx, (u32::from(func)); // FuncId
                        movq rax, (define_method);
                        call rax;
                    );
                }
                BcOp::Br(disp) => {
                    let dest = labels[(idx as i32 + 1 + disp) as usize];
                    monoasm!(self.jit,
                      jmp dest;
                    );
                }
                BcOp::CondBr(cond_, disp) => {
                    let cond_ = conv(cond_);
                    let dest = labels[(idx as i32 + 1 + disp) as usize];
                    monoasm!(self.jit,
                      movq rax, [rbp - (cond_)];
                      orq rax, 0x10;
                      cmpq rax, (FALSE_VALUE);
                      jne dest;
                    );
                }
                BcOp::CondNotBr(cond_, disp) => {
                    let cond_ = conv(cond_);
                    let dest = labels[(idx as i32 + 1 + disp) as usize];
                    monoasm!(self.jit,
                      cmpq rax, [rbp - (cond_)];
                      orq rax, 0x10;
                      cmpq rax, (FALSE_VALUE);
                      jeq dest;
                    );
                }
            }
        }
        label
    }
}
