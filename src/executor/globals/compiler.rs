use monoasm::*;
use monoasm_macro::monoasm;
use paste::paste;

use super::*;

mod jitgen;
mod vmgen;

type EntryPoint = extern "C" fn(&mut Executor, &mut Globals, *const FuncData) -> Option<Value>;

type Invoker = extern "C" fn(
    &mut Executor,
    &mut Globals,
    *const FuncData,
    Value,
    *const Value,
    usize,
) -> Option<Value>;

type Invoker2 =
    extern "C" fn(&mut Executor, &mut Globals, *const FuncData, Value, Arg, usize) -> Option<Value>;

///
/// Bytecode compiler
///
/// This generates x86-64 machine code from a bytecode.
///
pub struct Codegen {
    pub jit: JitMemory,
    pub class_version: DestLabel,
    pub class_version_addr: *mut u32,
    pub const_version: DestLabel,
    pub entry_panic: DestLabel,
    pub vm_entry: DestLabel,
    pub vm_fetch: DestLabel,
    pub entry_point: EntryPoint,
    entry_find_method: DestLabel,
    pub vm_return: DestLabel,
    pub f64_to_val: DestLabel,
    pub heap_to_f64: DestLabel,
    pub div_by_zero: DestLabel,
    pub dispatch: Vec<CodePtr>,
    pub method_invoker: Invoker,
    pub method_invoker2: Invoker2,
    pub block_invoker: Invoker,
}

fn conv(reg: SlotId) -> i64 {
    reg.0 as i64 * 8 + OFFSET_SELF
}

type WriteBack = Vec<(u16, Vec<SlotId>)>;

///
/// Context of the current Basic block.
///
#[derive(Debug, Clone, PartialEq)]
struct BBContext {
    /// information for stack slots.
    stack_slot: StackSlotInfo,
    /// information for xmm registers.
    xmm: [Vec<SlotId>; 14],
}

impl BBContext {
    fn new(reg_num: usize) -> Self {
        let xmm = (0..14)
            .map(|_| vec![])
            .collect::<Vec<Vec<SlotId>>>()
            .try_into()
            .unwrap();
        Self {
            stack_slot: StackSlotInfo(vec![LinkMode::None; reg_num]),
            xmm,
        }
    }

    fn from(slot_info: &StackSlotInfo) -> Self {
        let mut ctx = Self::new(slot_info.0.len());
        for (i, mode) in slot_info.0.iter().enumerate() {
            let reg = SlotId(i as u16);
            match mode {
                LinkMode::None => {}
                LinkMode::XmmR(x) => {
                    ctx.stack_slot[reg] = LinkMode::XmmR(*x);
                    ctx.xmm[*x as usize].push(reg);
                }
                LinkMode::XmmRW(x) => {
                    ctx.stack_slot[reg] = LinkMode::XmmRW(*x);
                    ctx.xmm[*x as usize].push(reg);
                }
            }
        }
        ctx
    }

    fn remove_unused(&mut self, unused: &[SlotId]) {
        unused.iter().for_each(|reg| self.dealloc_xmm(*reg));
    }

    ///
    /// Allocate a new xmm register.
    ///
    fn alloc_xmm(&mut self) -> u16 {
        for (flhs, xmm) in self.xmm.iter_mut().enumerate() {
            if xmm.is_empty() {
                return flhs as u16;
            }
        }
        unreachable!()
    }

    fn link_rw_xmm(&mut self, reg: SlotId, freg: u16) {
        self.stack_slot[reg] = LinkMode::XmmRW(freg);
        self.xmm[freg as usize].push(reg);
    }

    fn link_r_xmm(&mut self, reg: SlotId, freg: u16) {
        self.stack_slot[reg] = LinkMode::XmmR(freg);
        self.xmm[freg as usize].push(reg);
    }

    ///
    /// Deallocate an xmm register corresponding to the stack slot *reg*.
    ///
    fn dealloc_xmm(&mut self, reg: SlotId) {
        match self.stack_slot[reg] {
            LinkMode::XmmR(freg) | LinkMode::XmmRW(freg) => {
                assert!(self.xmm[freg as usize].contains(&reg));
                self.xmm[freg as usize].retain(|e| *e != reg);
                self.stack_slot[reg] = LinkMode::None;
            }
            LinkMode::None => {}
        }
    }

    fn xmm_swap(&mut self, l: u16, r: u16) {
        self.xmm.swap(l as usize, r as usize);
        self.stack_slot.0.iter_mut().for_each(|mode| match mode {
            LinkMode::XmmR(x) | LinkMode::XmmRW(x) => {
                if *x == l {
                    *x = r;
                } else if *x == r {
                    *x = l;
                }
            }
            LinkMode::None => {}
        });
    }

    ///
    /// Allocate new xmm register to the given stack slot for read/write f64.
    ///
    fn xmm_write(&mut self, reg: SlotId) -> u16 {
        if let LinkMode::XmmRW(freg) = self.stack_slot[reg] {
            if self.xmm[freg as usize].len() == 1 {
                assert_eq!(reg, self.xmm[freg as usize][0]);
                return freg;
            }
        };
        self.dealloc_xmm(reg);
        let freg = self.alloc_xmm();
        self.link_rw_xmm(reg, freg);
        freg
    }

    ///
    /// Allocate new xmm register to the given stack slot for read f64.
    ///
    fn alloc_xmm_read(&mut self, reg: SlotId) -> u16 {
        match self.stack_slot[reg] {
            LinkMode::None => {
                let freg = self.alloc_xmm();
                self.link_r_xmm(reg, freg);
                freg
            }
            _ => unreachable!(),
        }
    }

    ///
    /// Copy *src* to *dst*.
    ///
    fn copy_slot(&mut self, codegen: &mut Codegen, src: SlotId, dst: SlotId) {
        self.dealloc_xmm(dst);
        match self.stack_slot[src] {
            LinkMode::XmmRW(freg) | LinkMode::XmmR(freg) => {
                self.link_rw_xmm(dst, freg);
            }
            LinkMode::None => {
                monoasm!(codegen.jit,
                  movq rax, [rbp - (conv(src))];
                  movq [rbp - (conv(dst))], rax;
                );
            }
        }
    }

    ///
    /// Write back a corresponding xmm register to the stack slot *reg*.
    ///
    /// the xmm will be deallocated.
    ///
    fn write_back_slot(&mut self, codegen: &mut Codegen, reg: SlotId) {
        if let LinkMode::XmmRW(freg) = self.stack_slot[reg] {
            let f64_to_val = codegen.f64_to_val;
            monoasm!(codegen.jit,
                movq xmm0, xmm(freg as u64 + 2);
                call f64_to_val;
            );
            codegen.store_rax(reg);
            self.stack_slot[reg] = LinkMode::XmmR(freg);
        }
    }

    fn write_back_range(&mut self, codegen: &mut Codegen, arg: SlotId, len: u16) {
        for reg in arg.0..arg.0 + len {
            self.write_back_slot(codegen, SlotId::new(reg))
        }
    }

    fn get_write_back(&self) -> WriteBack {
        self.xmm
            .iter()
            .enumerate()
            .filter_map(|(i, v)| {
                if v.is_empty() {
                    None
                } else {
                    let v: Vec<_> = self.xmm[i]
                        .iter()
                        .filter(|reg| matches!(self.stack_slot[**reg], LinkMode::XmmRW(_)))
                        .cloned()
                        .collect();
                    Some((i as u16, v))
                }
            })
            .filter(|(_, v)| !v.is_empty())
            .collect()
    }

    fn get_xmm_using(&self) -> Vec<usize> {
        self.xmm
            .iter()
            .enumerate()
            .filter_map(|(i, v)| if v.is_empty() { None } else { Some(i) })
            .collect()
    }
}

#[derive(Debug)]
struct BranchEntry {
    src_idx: usize,
    bbctx: BBContext,
    dest_label: DestLabel,
}

pub(super) struct CompileContext {
    labels: HashMap<usize, DestLabel>,
    /// (bb_id, Vec<src_idx>)
    bb_info: Vec<Option<(usize, Vec<usize>)>>,
    bb_pos: usize,
    loop_count: usize,
    is_loop: bool,
    branch_map: HashMap<usize, Vec<BranchEntry>>,
    backedge_map: HashMap<usize, (DestLabel, StackSlotInfo, Vec<SlotId>)>,
    start_codepos: usize,
    #[cfg(feature = "emit-asm")]
    pub(super) sourcemap: Vec<(usize, usize)>,
}

impl CompileContext {
    pub(super) fn new(
        func: &ISeqInfo,
        codegen: &mut Codegen,
        start_pos: usize,
        is_loop: bool,
    ) -> Self {
        let bb_info = func.get_bb_info();
        let mut labels = HashMap::default();
        bb_info.into_iter().enumerate().for_each(|(idx, elem)| {
            if elem.is_some() {
                labels.insert(idx, codegen.jit.label());
            }
        });
        Self {
            labels,
            bb_info: func.get_bb_info(),
            bb_pos: start_pos,
            loop_count: 0,
            is_loop,
            branch_map: HashMap::default(),
            backedge_map: HashMap::default(),
            start_codepos: 0,
            #[cfg(feature = "emit-asm")]
            sourcemap: vec![],
        }
    }

    fn new_branch(&mut self, src_idx: usize, dest: usize, bbctx: BBContext, dest_label: DestLabel) {
        self.branch_map.entry(dest).or_default().push(BranchEntry {
            src_idx,
            bbctx,
            dest_label,
        })
    }

    fn new_backedge(
        &mut self,
        bb_pos: usize,
        dest_label: DestLabel,
        slot_info: StackSlotInfo,
        unused: Vec<SlotId>,
    ) {
        self.backedge_map
            .insert(bb_pos, (dest_label, slot_info, unused));
    }

    fn get_backedge(&mut self, bb_pos: usize) -> (DestLabel, StackSlotInfo, Vec<SlotId>) {
        self.backedge_map.remove_entry(&bb_pos).unwrap().1
    }
}

///
/// Mode of linkage between stack slot and xmm registers.
///
#[derive(Debug, Clone, Copy, PartialEq)]
enum LinkMode {
    ///
    /// Linked to an xmm register and we can read and write.
    ///
    /// mutation of the corresponding xmm register (lazily) affects the stack slot.
    ///
    XmmRW(u16),
    ///
    /// Linked to an xmm register but we can only read.
    ///
    XmmR(u16),
    ///
    /// No linkage with any xmm regiter.
    ///
    None,
}

#[derive(Clone, PartialEq)]
struct StackSlotInfo(Vec<LinkMode>);

impl std::fmt::Debug for StackSlotInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s: String = self
            .0
            .iter()
            .enumerate()
            .flat_map(|(i, mode)| match mode {
                LinkMode::None => None,
                LinkMode::XmmR(x) => Some(format!("%{i}:R({}) ", x)),
                LinkMode::XmmRW(x) => Some(format!("%{i}:RW({}) ", x)),
            })
            .collect();
        write!(f, "[{s}]")
    }
}

impl std::ops::Index<SlotId> for StackSlotInfo {
    type Output = LinkMode;
    fn index(&self, i: SlotId) -> &Self::Output {
        &self.0[i.0 as usize]
    }
}

impl std::ops::IndexMut<SlotId> for StackSlotInfo {
    fn index_mut(&mut self, i: SlotId) -> &mut Self::Output {
        &mut self.0[i.0 as usize]
    }
}

impl StackSlotInfo {
    fn merge(&mut self, other: &StackSlotInfo) {
        self.0.iter_mut().zip(other.0.iter()).for_each(|(l, r)| {
            *l = match (&l, r) {
                (LinkMode::XmmR(l), LinkMode::XmmR(_) | LinkMode::XmmRW(_))
                | (LinkMode::XmmRW(l), LinkMode::XmmR(_)) => LinkMode::XmmR(*l),
                (LinkMode::XmmRW(l), LinkMode::XmmRW(_)) => LinkMode::XmmRW(*l),
                _ => LinkMode::None,
            };
        });
    }

    fn merge_entries(entries: &[BranchEntry]) -> Self {
        let mut target = entries[0].bbctx.stack_slot.clone();
        #[cfg(feature = "emit-tir")]
        eprintln!("  <-{}: {:?}", entries[0].src_idx, target);
        for BranchEntry {
            src_idx: _src_idx,
            bbctx,
            dest_label: _,
        } in entries.iter().skip(1)
        {
            #[cfg(feature = "emit-tir")]
            eprintln!("  <-{_src_idx}: {:?}", bbctx.stack_slot);
            target.merge(&bbctx.stack_slot);
        }
        target
    }
}

//
// Runtime functions.
//

///
/// Get an absolute address of the given method.
///
/// If no method was found, return None (==0u64).
///
extern "C" fn find_method<'a>(
    globals: &'a mut Globals,
    func_name: IdentId,
    args_len: usize,
    receiver: Value,
) -> Option<&'a FuncData> {
    let func_id = globals.find_method_checked(receiver, func_name, args_len)?;
    let data = globals.compile_on_demand(func_id);
    Some(data)
}

extern "C" fn vm_get_func_data<'a>(globals: &'a mut Globals, func_id: FuncId) -> &'a FuncData {
    globals.compile_on_demand(func_id)
}

extern "C" fn gen_array(src: *const Value, len: usize) -> Value {
    let mut v = if len == 0 {
        vec![]
    } else {
        unsafe { std::slice::from_raw_parts(src.sub(len - 1), len).to_vec() }
    };
    v.reverse();
    Value::new_array(v)
}

extern "C" fn get_index(
    interp: &mut Executor,
    globals: &mut Globals,
    mut base: Value,
    index: Value,
) -> Option<Value> {
    match base.class_id() {
        ARRAY_CLASS => {
            if let Some(idx) = index.try_fixnum() {
                let v = base.as_array_mut();
                return Some(if idx >= 0 {
                    v.get(idx as usize).cloned().unwrap_or_default()
                } else {
                    let idx = v.len() as i64 + idx;
                    if idx < 0 {
                        Value::nil()
                    } else {
                        v.get(idx as usize).cloned().unwrap_or_default()
                    }
                });
            }
        }
        _ => {}
    }
    interp.invoke_method(globals, IdentId::_INDEX, base, &[index])
}

extern "C" fn set_index(
    interp: &mut Executor,
    globals: &mut Globals,
    mut base: Value,
    index: Value,
    src: Value,
) -> Option<Value> {
    match base.class_id() {
        ARRAY_CLASS => {
            let v = base.as_array_mut();
            if let Some(idx) = index.try_fixnum() {
                if idx >= 0 {
                    match v.get_mut(idx as usize) {
                        Some(v) => *v = src,
                        None => {
                            let idx = idx as usize;
                            v.extend((v.len()..idx).into_iter().map(|_| Value::nil()));
                            v.push(src);
                        }
                    }
                } else {
                    let idx_positive = v.len() as i64 + idx;
                    if idx_positive < 0 {
                        globals.err_index_too_small(idx, -(v.len() as i64));
                        return None;
                    } else {
                        v[idx_positive as usize] = src;
                    }
                };
                return Some(src);
            }
        }
        _ => {}
    }
    interp.invoke_method(globals, IdentId::_INDEX_ASSIGN, base, &[index, src])
}

extern "C" fn get_instance_var(base: Value, name: IdentId, globals: &mut Globals) -> Value {
    globals.get_ivar(base, name).unwrap_or_default()
}

extern "C" fn set_instance_var(
    globals: &mut Globals,
    base: Value,
    name: IdentId,
    val: Value,
) -> Option<Value> {
    globals.set_ivar(base, name, val)?;
    Some(val)
}

extern "C" fn define_class(
    interp: &mut Executor,
    globals: &mut Globals,
    name: IdentId,
    superclass: Option<Value>,
) -> Option<Value> {
    let parent = interp.get_class_context();
    let self_val = match globals.get_constant(parent, name) {
        Some(val) => {
            let class = val.expect_class(name, globals)?;
            if let Some(superclass) = superclass {
                let super_name = globals.val_tos(superclass);
                let super_name = IdentId::get_ident_id(&super_name);
                let super_class = superclass.expect_class(super_name, globals)?;
                if Some(super_class) != class.super_class(globals) {
                    globals.err_superclass_mismatch(name);
                    return None;
                }
            }
            val
        }
        None => {
            let superclass = match superclass {
                Some(superclass) => {
                    let name = globals.val_tos(superclass);
                    let name = IdentId::get_ident_id_from_string(name);
                    superclass.expect_class(name, globals)?
                }
                None => OBJECT_CLASS,
            };
            globals.define_class_by_ident_id(name, Some(superclass), parent)
        }
    };
    //globals.get_singleton_id(self_val.as_class());
    interp.push_class_context(self_val.as_class());
    Some(self_val)
}

extern "C" fn pop_class_context(interp: &mut Executor, _globals: &mut Globals) {
    interp.pop_class_context();
}

extern "C" fn unimplemented_inst(_: &mut Executor, _: &mut Globals, opcode: u64) {
    panic!("unimplemented inst. {:016x}", opcode);
}

extern "C" fn panic(_: &mut Executor, _: &mut Globals) {
    panic!("panic in jit code.");
}

/*pub extern "C" fn eprintln(rdi: u64, rsi: u64) {
    eprintln!("rdi:{:016x} rsi:{:016x}", rdi, rsi);
}*/

extern "C" fn error_divide_by_zero(globals: &mut Globals) {
    globals.err_divide_by_zero();
}

extern "C" fn get_error_location(
    _interp: &mut Executor,
    globals: &mut Globals,
    meta: Meta,
    pc: BcPc,
) {
    let func_info = &globals.func[meta.func_id()];
    let bc_base = func_info.data.pc;
    let normal_info = match &func_info.kind {
        FuncKind::ISeq(info) => info,
        FuncKind::Builtin { .. } => return,
        FuncKind::AttrReader { .. } => return,
        FuncKind::AttrWriter { .. } => return,
    };
    let sourceinfo = normal_info.sourceinfo.clone();
    let loc = normal_info.sourcemap[pc - bc_base];
    globals.push_error_location(loc, sourceinfo);
}

impl Codegen {
    pub(crate) fn new(no_jit: bool, main_object: Value) -> Self {
        let mut jit = JitMemory::new();
        let class_version = jit.const_i32(0);
        let const_version = jit.const_i64(0);
        let entry_panic = jit.label();
        let entry_find_method = jit.label();
        let jit_return = jit.label();
        let vm_return = jit.label();
        let div_by_zero = jit.label();
        let heap_to_f64 = jit.label();
        //jit.select_page(1);
        monoasm!(&mut jit,
        entry_panic:
            movq rdi, rbx;
            movq rsi, r12;
            movq rax, (op::_dump_stacktrace);
            subq rax, 8;
            call rax;
            addq rax, 8;
            movq rdi, rbx;
            movq rsi, r12;
            movq rax, (panic);
            jmp rax;
        entry_find_method:
            movq rdi, r12;
            movq rax, (find_method);
            jmp  rax;
        vm_return:
            movq r15, rax;
            movq rdi, rbx;
            movq rsi, r12;
            movq rdx, [rbp - (OFFSET_META)];
            movq rcx, r13;
            subq rcx, 8;
            movq rax, (get_error_location);
            call rax;
            // restore return value
            movq rax, r15;
        jit_return:
            leave;
            ret;
        div_by_zero:
            movq rdi, r12;
            movq rax, (error_divide_by_zero);
            call rax;
            xorq rax, rax;
            leave;
            ret;
        heap_to_f64:
            // we must save rdi for log_optimize.
            subq rsp, 128;
            movq [rsp + 112], rdi;
            movq [rsp + 104], xmm15;
            movq [rsp + 96], xmm14;
            movq [rsp + 88], xmm13;
            movq [rsp + 80], xmm12;
            movq [rsp + 72], xmm11;
            movq [rsp + 64], xmm10;
            movq [rsp + 56], xmm9;
            movq [rsp + 48], xmm8;
            movq [rsp + 40], xmm7;
            movq [rsp + 32], xmm6;
            movq [rsp + 24], xmm5;
            movq [rsp + 16], xmm4;
            movq [rsp + 8], xmm3;
            movq [rsp + 0], xmm2;
            movq rax, (Value::val_tof);
            call rax;
            movq xmm2, [rsp + 0];
            movq xmm3, [rsp + 8];
            movq xmm4, [rsp + 16];
            movq xmm5, [rsp + 24];
            movq xmm6, [rsp + 32];
            movq xmm7, [rsp + 40];
            movq xmm8, [rsp + 48];
            movq xmm9, [rsp + 56];
            movq xmm10, [rsp + 64];
            movq xmm11, [rsp + 72];
            movq xmm12, [rsp + 80];
            movq xmm13, [rsp + 88];
            movq xmm14, [rsp + 96];
            movq xmm15, [rsp + 104];
            movq rdi, [rsp + 112];
            addq rsp, 128;
            ret;
        );

        fn gen_invoker_prologue(mut jit: &mut JitMemory, invoke_block: bool) {
            monoasm! { jit,
                pushq rbx;
                pushq r12;
                pushq r13;
                pushq r14;
                pushq r15;
                movq rbx, rdi;
                movq r12, rsi;
                // set meta/func_id
                movq rax, [rdx + (FUNCDATA_OFFSET_META)];
                movq [rsp - (16 + OFFSET_META)], rax;
                movq [rsp - (16 + OFFSET_BLOCK)], 0;
                // push frame
                movq rax, [rbx];
                lea  rdi, [rsp - (16 + OFFSET_CFP)];
                movq [rdi], rax;
                movq [rbx], rdi;
            };
            if invoke_block {
                monoasm! { jit,
                    movq rax, [rax];
                    lea  rax, [rax - ((OFFSET_OUTER - OFFSET_CFP) as i32)];
                    movq [rsp - (16 + OFFSET_OUTER)], rax;
                };
            } else {
                monoasm! { jit,
                    movq [rsp - (16 + OFFSET_OUTER)], 0;
                };
            }
            monoasm! { jit,
                // set self (= receiver)
                movq [rsp - (16 + OFFSET_SELF)], rcx;

                movq r13, [rdx + (FUNCDATA_OFFSET_PC)];    // r13: BcPc
                //
                //       +-------------+
                // +0x08 |             |
                //       +-------------+
                //  0x00 |             | <- rsp
                //       +-------------+
                // -0x08 | return addr |
                //       +-------------+
                // -0x10 |   old rbp   |
                //       +-------------+
                // -0x18 |    outer    |
                //       +-------------+
                // -0x20 |    meta     | func_id
                //       +-------------+
                // -0x28 |    Block    |
                //       +-------------+
                // -0x30 |     %0      | receiver
                //       +-------------+
                // -0x38 | %1(1st arg) |
                //       +-------------+
                //       |             |
                //
            };
        }

        fn gen_invoker_epilogue(mut jit: &mut JitMemory) {
            monoasm! { jit,
                movq rax, [rdx + (FUNCDATA_OFFSET_CODEPTR)];
                call rax;
                movq rdi, [rsp - (16 + OFFSET_CFP)];
                movq [rbx], rdi;
                popq r15;
                popq r14;
                popq r13;
                popq r12;
                popq rbx;
                ret;
            };
        }

        fn gen_invoker_prep(mut jit: &mut JitMemory) {
            let loop_exit = jit.label();
            let loop_ = jit.label();
            monoasm! { &mut jit,
                // r8 <- *args
                // r9 <- len
                movq rdi, r9;
                testq r9, r9;
                jeq  loop_exit;
                movq r10, r9;
                negq r9;
            loop_:
                movq rax, [r8 + r10 * 8 - 8];
                movq [rsp + r9 * 8 - (16 + OFFSET_SELF)], rax;
                subq r10, 1;
                addq r9, 1;
                jne  loop_;
            loop_exit:
            };
        }

        // method invoker.
        let method_invoker: extern "C" fn(
            &mut Executor,
            &mut Globals,
            *const FuncData,
            Value,
            *const Value,
            usize,
        ) -> Option<Value> = unsafe { std::mem::transmute(jit.get_current_address().as_ptr()) };
        // rdi: &mut Interp
        // rsi: &mut Globals
        // rdx: *const FuncData
        // rcx: receiver: Value
        // r8:  *args: *const Value
        // r9:  len: usize

        gen_invoker_prologue(&mut jit, false);
        gen_invoker_prep(&mut jit);
        gen_invoker_epilogue(&mut jit);

        // block invoker.
        let block_invoker: extern "C" fn(
            &mut Executor,
            &mut Globals,
            *const FuncData,
            Value,
            *const Value,
            usize,
        ) -> Option<Value> = unsafe { std::mem::transmute(jit.get_current_address().as_ptr()) };
        gen_invoker_prologue(&mut jit, true);
        gen_invoker_prep(&mut jit);
        gen_invoker_epilogue(&mut jit);

        // method invoker.
        let method_invoker2: extern "C" fn(
            &mut Executor,
            &mut Globals,
            *const FuncData,
            Value,
            Arg,
            usize,
        ) -> Option<Value> = unsafe { std::mem::transmute(jit.get_current_address().as_ptr()) };
        let loop_exit = jit.label();
        let loop_ = jit.label();
        // rdi: &mut Interp
        // rsi: &mut Globals
        // rdx: *const FuncData
        // rcx: receiver: Value
        // r8:  args: Arg
        // r9:  len: usize

        gen_invoker_prologue(&mut jit, false);
        monoasm! { &mut jit,
            // r8 <- *args
            // r9 <- len
            movq rdi, r9;
            testq r9, r9;
            jeq  loop_exit;
            negq r9;
        loop_:
            movq rax, [r8 + r9 * 8 + 8];
            movq [rsp + r9 * 8 - (16 + OFFSET_SELF)], rax;
            addq r9, 1;
            jne  loop_;
        loop_exit:
        };
        gen_invoker_epilogue(&mut jit);

        // dispatch table.
        let entry_unimpl = jit.get_current_address();
        monoasm! { jit,
                movq rdi, rbx;
                movq rsi, r12;
                movq rdx, [r13 - 16];
                movq rax, (super::compiler::unimplemented_inst);
                call rax;
                leave;
                ret;
        };
        //jit.select_page(0);
        let dispatch = vec![entry_unimpl; 256];
        let mut codegen = Self {
            jit,
            class_version,
            class_version_addr: std::ptr::null_mut(),
            const_version,
            entry_panic,
            entry_find_method,
            vm_entry: entry_panic,
            vm_fetch: entry_panic,
            entry_point: unsafe { std::mem::transmute(entry_unimpl.as_ptr()) },
            vm_return,
            f64_to_val: entry_panic,
            heap_to_f64,
            div_by_zero,
            dispatch,
            method_invoker,
            method_invoker2,
            block_invoker,
        };
        codegen.f64_to_val = codegen.generate_f64_to_val();
        codegen.construct_vm(no_jit);
        codegen.gen_entry_point(main_object);
        codegen.jit.finalize();
        codegen.class_version_addr =
            codegen.jit.get_label_address(class_version).as_ptr() as *mut u32;
        codegen
    }

    /// Push frame
    ///
    /// destroy rax, rdi
    fn push_frame(&mut self) {
        monoasm!(self.jit,
            movq rax, [rbx];
            lea  rdi, [rsp - (16 + OFFSET_CFP)];
            movq [rdi], rax;
            movq [rbx], rdi;
        );
    }

    /// Pop frame
    ///
    /// destroy rdi
    fn pop_frame(&mut self) {
        monoasm!(self.jit,
            movq rdi, [rsp - (16 + OFFSET_CFP)];
            movq [rbx], rdi;
        );
    }

    ///
    /// calculate an offset of stack pointer.
    ///
    fn calc_offset(&mut self) {
        monoasm!(self.jit,
            addq rax, (OFFSET_ARG0 / 8 + 1);
            andq rax, (-2);
            shlq rax, 3;
        );
    }

    ///
    /// check whether lhs and rhs are fixnum.
    ///
    fn guard_rdi_rsi_fixnum(&mut self, generic: DestLabel) {
        self.guard_rdi_fixnum(generic);
        self.guard_rsi_fixnum(generic);
    }

    ///
    /// check whether lhs is fixnum.
    ///
    fn guard_rdi_fixnum(&mut self, generic: DestLabel) {
        monoasm!(self.jit,
            testq rdi, 0x1;
            jz generic;
        );
    }

    ///
    /// check whether rhs is fixnum.
    ///
    fn guard_rsi_fixnum(&mut self, generic: DestLabel) {
        monoasm!(self.jit,
            testq rsi, 0x1;
            jz generic;
        );
    }

    ///
    /// store rax to *ret*.
    ///
    fn store_rax(&mut self, ret: SlotId) {
        monoasm!(self.jit,
            movq [rbp - (conv(ret))], rax;
        );
    }

    ///
    /// store rdi to *ret*.
    ///
    fn store_rdi(&mut self, ret: SlotId) {
        monoasm!(self.jit,
            movq [rbp - (conv(ret))], rdi;
        );
    }

    ///
    /// store rsi to *ret*.
    ///
    fn store_rsi(&mut self, ret: SlotId) {
        monoasm!(self.jit,
            // store the result to return reg.
            movq [rbp - (conv(ret))], rsi;
        );
    }

    ///
    /// move xmm(*src*) to xmm(*dst*).
    ///
    fn xmm_mov(&mut self, src: u16, dst: u16) {
        if src != dst {
            monoasm!(self.jit,
                movq  xmm(dst as u64 + 2), xmm(src as u64 + 2);
            );
        }
    }

    ///
    /// Assume the Value is Integer, and convert to f64.
    ///
    /// side-exit if not Integer.
    ///
    /// ### in
    ///
    /// - rdi: Value
    ///
    /// ### out
    ///
    /// - xmm0: f64
    ///
    fn gen_val_to_f64_assume_integer(&mut self, xmm: u64, side_exit: DestLabel) -> DestLabel {
        let entry = self.jit.label();
        monoasm!(&mut self.jit,
        entry:
            testq rdi, 0b01;
            jz side_exit;
            sarq rdi, 1;
            cvtsi2sdq xmm(xmm), rdi;
        );
        entry
    }

    ///
    /// Assume the Value is Float, and convert to f64.
    ///
    /// side-exit if not Float.
    ///
    /// ### in
    ///
    /// - rdi: Value
    ///
    /// ### out
    ///
    /// - xmm(*xmm*): f64
    ///
    /// ### registers destroyed
    ///
    /// - rax, rdi
    ///
    fn gen_val_to_f64_assume_float(&mut self, xmm: u64, side_exit: DestLabel) -> DestLabel {
        let heap_to_f64 = self.heap_to_f64;
        let entry = self.jit.label();
        let heap = self.jit.label();
        let exit = self.jit.label();
        monoasm!(&mut self.jit,
        entry:
            testq rdi, 0b01;
            jnz side_exit;
            testq rdi, 0b10;
            jz heap;
            xorps xmm(xmm), xmm(xmm);
            movq rax, (FLOAT_ZERO);
            cmpq rdi, rax;
            je exit;
            movq rax, rdi;
            sarq rax, 63;
            addq rax, 2;
            andq rdi, (-4);
            orq rdi, rax;
            rolq rdi, 61;
            movq xmm(xmm), rdi;
            jmp exit;
        heap:
            call heap_to_f64;
            testq rax, rax;
            jz   side_exit;
            movq xmm(xmm), xmm0;
        exit:
        );

        entry
    }

    ///
    /// Confirm the Value is Float.
    ///
    /// side-exit if not Float.
    ///
    /// ### registers destroyed
    ///
    /// - rax, rdi
    ///
    pub(crate) fn gen_assume_float(&mut self, reg: SlotId, side_exit: DestLabel) {
        let heap_to_f64 = self.heap_to_f64;
        let heap = self.jit.label();
        let exit = self.jit.label();
        monoasm!(&mut self.jit,
            movq rdi, [rbp - (conv(reg))];
            testq rdi, 0b01;
            jnz side_exit;
            testq rdi, 0b10;
            jnz exit;
        heap:
            call heap_to_f64;
            testq rax, rax;
            jz   side_exit;
        exit:
        );
    }

    ///
    /// Convert the Value to f64.
    ///
    /// side-exit if neither Float nor Integer.
    ///
    /// ### in
    ///
    /// - rdi: Value
    ///
    /// ### out
    ///
    /// - xmm(*xmm*): f64
    ///
    /// ### registers destroyed
    ///
    /// - caller save registers except xmm registers(xmm2-xmm15).
    ///
    fn gen_val_to_f64(&mut self, xmm: u64, side_exit: DestLabel) {
        let heap_to_f64 = self.heap_to_f64;
        let integer = self.jit.label();
        let heap = self.jit.label();
        let exit = self.jit.label();
        monoasm!(&mut self.jit,
            testq rdi, 0b01;
            jnz integer;
            testq rdi, 0b10;
            jz  heap;
            xorps xmm(xmm), xmm(xmm);
            movq rax, (FLOAT_ZERO);
            cmpq rdi, rax;
            je  exit;
            movq rax, rdi;
            sarq rax, 63;
            addq rax, 2;
            andq rdi, (-4);
            orq rdi, rax;
            rolq rdi, 61;
            movq xmm(xmm), rdi;
            jmp exit;
        integer:
            sarq rdi, 1;
            cvtsi2sdq xmm(xmm), rdi;
            jmp exit;
        heap:
            call heap_to_f64;
            testq rax, rax;
            jz   side_exit;
            movq xmm(xmm), xmm0;
        exit:
        );
    }

    ///
    /// Convert f64 to Value.
    ///
    /// ### in
    ///
    /// - xmm0: f64
    ///
    /// ### out
    ///
    /// - rax: Value
    ///
    /// ### registers destroyed
    ///
    /// - rcx, xmm1
    ///
    fn generate_f64_to_val(&mut self) -> DestLabel {
        let entry = self.jit.label();
        let normal = self.jit.label();
        let heap_alloc = self.jit.label();
        monoasm!(self.jit,
        entry:
            xorps xmm1, xmm1;
            ucomisd xmm0, xmm1;
            jne normal;
            jp normal;
            movq rax, (Value::new_float(0.0).get());
            ret;
        heap_alloc:
        // we must save rdi for log_optimize.
            subq rsp, 120;
            movq [rsp + 112], rdi;
            movq [rsp + 104], xmm15;
            movq [rsp + 96], xmm14;
            movq [rsp + 88], xmm13;
            movq [rsp + 80], xmm12;
            movq [rsp + 72], xmm11;
            movq [rsp + 64], xmm10;
            movq [rsp + 56], xmm9;
            movq [rsp + 48], xmm8;
            movq [rsp + 40], xmm7;
            movq [rsp + 32], xmm6;
            movq [rsp + 24], xmm5;
            movq [rsp + 16], xmm4;
            movq [rsp + 8], xmm3;
            movq [rsp + 0], xmm2;
            movq rax, (Value::new_float);
            call rax;
            movq xmm2, [rsp + 0];
            movq xmm3, [rsp + 8];
            movq xmm4, [rsp + 16];
            movq xmm5, [rsp + 24];
            movq xmm6, [rsp + 32];
            movq xmm7, [rsp + 40];
            movq xmm8, [rsp + 48];
            movq xmm9, [rsp + 56];
            movq xmm10, [rsp + 64];
            movq xmm11, [rsp + 72];
            movq xmm12, [rsp + 80];
            movq xmm13, [rsp + 88];
            movq xmm14, [rsp + 96];
            movq xmm15, [rsp + 104];
            movq rdi, [rsp + 112];
            addq rsp, 120;
            ret;
        normal:
            movq rax, xmm0;
            movq rcx, rax;
            shrq rcx, 60;
            addl rcx, 1;
            andl rcx, 6;
            cmpl rcx, 4;
            jne heap_alloc;
            rolq rax, 3;
            andq rax, (-4);
            orq rax, 2;
            ret;
        );
        entry
    }

    fn call_unop(&mut self, func: usize) {
        monoasm!(self.jit,
            movq rdx, rdi;
            movq rdi, rbx;
            movq rsi, r12;
            movq rax, (func);
            call rax;
        );
    }

    fn call_binop(&mut self, func: usize) {
        monoasm!(self.jit,
            movq rdx, rdi;
            movq rcx, rsi;
            movq rdi, rbx;
            movq rsi, r12;
            movq rax, (func);
            call rax;
        );
    }

    ///
    /// Set jit compilation stub code for an entry point of each Ruby methods.
    ///
    pub(super) fn gen_jit_stub(&mut self) -> CodePtr {
        let vm_entry = self.vm_entry;
        let codeptr = self.jit.get_current_address();
        let counter = self.jit.const_i32(5);
        let entry = self.jit.label();
        monoasm!(self.jit,
        entry:
            subl [rip + counter], 1;
            jne vm_entry;
            movl rsi, [rsp - (8 + OFFSET_FUNCID)];
            movq rdx, [rsp - (8 + OFFSET_SELF)];
            subq rsp, 1024;
            pushq rdi;
            movq rdi, r12;
            movq rax, (Self::exec_jit_compile);
            call rax;
            movw [rip + entry], 0xe9;   // jmp
            lea rdi, [rip + entry];
            addq rdi, 5;
            subq rax, rdi;
            movl [rdi - 4], rax;
            popq rdi;
            addq rsp, 1024;
            jmp entry;
        );
        codeptr
    }

    pub(super) fn gen_vm_stub(&mut self) -> CodePtr {
        let vm_entry = self.vm_entry;
        let codeptr = self.jit.get_current_address();
        monoasm!(self.jit,
            jmp vm_entry;
        );
        codeptr
    }

    ///
    /// Generate a wrapper for a native function with C ABI.
    ///
    /// - stack layout at the point of just after a wrapper was called.
    /// ~~~text
    ///       +-------------+
    ///  0x00 | return addr | <- rsp
    ///       +-------------+
    /// -0x08 |             |
    ///       +-------------+
    /// -0x10 |    meta     |
    ///       +-------------+
    /// -0x18 |  %0 (self)  |
    ///       +-------------+
    /// -0x20 | %1(1st arg) |
    ///       +-------------+
    ///
    ///  meta
    /// +-------------------+ -0x08
    /// |     2:Native      |
    /// +-------------------+ -0x0a
    /// |    register_len   |
    /// +-------------------+ -0x0c
    /// |                   |
    /// +      FuncId       + -0x0e
    /// |                   |
    /// +-------------------+ -0x10
    ///
    /// argument registers:
    ///   rdi: number of args
    ///
    /// global registers:
    ///   rbx: &mut Interp
    ///   r12: &mut Globals
    ///   r13: pc (dummy for builtin funcions)
    /// ~~~
    ///
    pub(super) fn wrap_native_func(&mut self, abs_address: u64) -> CodePtr {
        let label = self.jit.get_current_address();
        // calculate stack offset
        monoasm!(self.jit,
            pushq rbp;
            movq rbp, rsp;
            movq r8, rdi;
            movq rax, rdi;
        );
        self.calc_offset();
        monoasm!(self.jit,
            subq rsp, rax;
            lea  rcx, [rbp - (OFFSET_ARG0)];     // rcx <- *const arg[0]
            movq  r9, [rbp - (OFFSET_BLOCK)];     // r9 <- block
            movq  rdx, [rbp - (OFFSET_SELF)];    // rdx <- self
            // we should overwrite reg_num because the func itself does not know actual number of arguments.
            movw [rbp - (OFFSET_REGNUM)], rdi;

            movq rdi, rbx;
            movq rsi, r12;
            movq rax, (abs_address);
            // fn(&mut Interp, &mut Globals, Value, *const Value, len:usize, block:Option<Value>)
            call rax;

            leave;
            ret;
        );
        label
    }

    ///
    /// Generate attr_reader.
    ///
    /// - stack layout at the point of just after being called.
    /// ~~~text
    ///       +-------------+
    ///  0x00 | return addr | <- rsp
    ///       +-------------+
    /// -0x08 |             |
    ///       +-------------+
    /// -0x10 |    meta     |
    ///       +-------------+
    /// -0x18 |  %0 (self)  |
    ///       +-------------+
    /// ~~~
    pub(super) fn gen_attr_reader(&mut self, ivar_name: IdentId) -> CodePtr {
        let label = self.jit.get_current_address();
        let cached_class = self.jit.const_i32(0);
        let cached_ivarid = self.jit.const_i32(0);
        monoasm!(self.jit,
            movq rdi, [rsp - (8 + OFFSET_SELF)];  // self: Value
            movq rsi, (ivar_name.get()); // name: IdentId
            movq rdx, r12; // &mut Globals
            lea  rcx, [rip + cached_class];
            lea  r8, [rip + cached_ivarid];
            movq rax, (get_instance_var_with_cache);
            subq rsp, 8;
            call rax;
            addq rsp, 8;
            ret;
        );
        label
    }

    ///
    /// Generate attr_writer.
    ///
    /// - stack layout at the point of just after being called.
    /// ~~~text
    ///       +-------------+
    ///  0x00 | return addr | <- rsp
    ///       +-------------+
    /// -0x08 |             |
    ///       +-------------+
    /// -0x10 |    meta     |
    ///       +-------------+
    /// -0x18 |  %0 (self)  |
    ///       +-------------+
    /// -0x20 |   %1(val)   |
    ///       +-------------+
    /// ~~~
    pub(super) fn gen_attr_writer(&mut self, ivar_name: IdentId) -> CodePtr {
        let label = self.jit.get_current_address();
        let cached_class = self.jit.const_i32(0);
        let cached_ivarid = self.jit.const_i32(0);
        monoasm!(self.jit,
            movq rdi, r12; //&mut Globals
            movq rsi, [rsp - (8 + OFFSET_SELF)];  // self: Value
            movq rdx, (ivar_name.get()); // name: IdentId
            movq rcx, [rsp - (8 + OFFSET_ARG0)];  //val: Value
            lea  r8, [rip + cached_class];
            lea  r9, [rip + cached_ivarid];
            movq rax, (set_instance_var_with_cache);
            subq rsp, 8;
            call rax;
            addq rsp, 8;
            ret;
        );
        label
    }
}

impl Codegen {
    ///
    /// Compile the Ruby method.
    ///
    extern "C" fn exec_jit_compile(
        globals: &mut Globals,
        func_id: FuncId,
        self_value: Value,
    ) -> CodePtr {
        globals.func[func_id].data.meta.set_jit();
        let self_class = self_value.class_id();
        let label = globals.jit_compile_ruby(func_id, self_class, None);
        globals.codegen.jit.get_label_address(label)
    }

    ///
    /// Compile the loop.
    ///
    extern "C" fn exec_jit_partial_compile(
        globals: &mut Globals,
        func_id: FuncId,
        pc: BcPc,
        self_value: Value,
    ) -> CodePtr {
        let pc_index = pc - globals.func[func_id].data.pc;
        let self_class = self_value.class_id();
        let label = globals.jit_compile_ruby(func_id, self_class, Some(pc_index));
        globals.codegen.jit.get_label_address(label)
    }

    pub(super) fn jit_compile_ruby(
        &mut self,
        fnstore: &FnStore,
        func_id: FuncId,
        self_class: ClassId,
        position: Option<usize>,
    ) -> (DestLabel, CompileContext) {
        let func = fnstore[func_id].as_ruby_func();
        let start_pos = position.unwrap_or_default();

        #[cfg(any(feature = "emit-asm", feature = "log-jit", feature = "emit-tir"))]
        {
            eprintln!(
                "--> start {} compile: {} {:?} self_class:{:?} start:[{:05}] bytecode:{:?}",
                if position.is_some() {
                    "partial"
                } else {
                    "whole"
                },
                match func.name() {
                    Some(name) => name,
                    None => "<unnamed>",
                },
                func.id,
                self_class,
                start_pos,
                func.bytecode().as_ptr(),
            );
        }
        #[cfg(any(feature = "emit-asm", feature = "log-jit"))]
        let now = std::time::Instant::now();

        let entry = self.jit.label();
        self.jit.bind_label(entry);

        let mut cc = CompileContext::new(func, self, start_pos, position.is_some());
        let bb_start_pos: Vec<_> = cc
            .bb_info
            .iter()
            .enumerate()
            .filter_map(|(idx, v)| match v {
                Some(_) => {
                    if idx >= start_pos {
                        Some(idx)
                    } else {
                        None
                    }
                }
                None => None,
            })
            .collect();
        let reg_num = func.total_reg_num();
        cc.start_codepos = self.jit.get_current();

        if position.is_none() {
            self.prologue(func.total_reg_num(), func.total_arg_num());
        }

        cc.branch_map.insert(
            start_pos,
            vec![BranchEntry {
                src_idx: 0,
                bbctx: BBContext::new(reg_num),
                dest_label: self.jit.label(),
            }],
        );
        for i in bb_start_pos {
            cc.bb_pos = i;
            if self.compile_bb(fnstore, func, &mut cc) {
                break;
            };
        }

        let keys: Vec<_> = cc.branch_map.keys().cloned().collect();
        for pos in keys.into_iter() {
            self.gen_backedge_branch(&mut cc, func, pos);
        }

        self.jit.finalize();

        #[cfg(any(feature = "emit-asm", feature = "log-jit"))]
        let elapsed = now.elapsed();
        //#[cfg(feature = "emit-tir")]
        //eprintln!("{:?}", cc.tir);

        #[cfg(any(feature = "emit-asm", feature = "log-jit"))]
        eprintln!("    finished compile. elapsed:{:?}", elapsed);
        #[cfg(feature = "emit-tir")]
        eprintln!("    finished compile.");
        (entry, cc)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn float_test() {
        let mut gen = Codegen::new(false, Value::nil());

        let panic = gen.entry_panic;
        let from_f64_entry = gen.jit.get_label_address(gen.f64_to_val);
        let assume_float_to_f64 = gen.jit.label();
        monoasm!(&mut gen.jit,
        assume_float_to_f64:
            pushq rbp;
        );
        gen.gen_val_to_f64_assume_float(0, panic);
        monoasm!(&mut gen.jit,
            popq rbp;
            ret;
        );
        gen.jit.finalize();
        let to_f64_entry = gen.jit.get_label_address(assume_float_to_f64);

        let from_f64: fn(f64) -> Value = unsafe { std::mem::transmute(from_f64_entry.as_ptr()) };
        let to_f64: fn(Value) -> f64 = unsafe { std::mem::transmute(to_f64_entry.as_ptr()) };

        for n in [
            0.0,
            4.2,
            35354354354.2135365,
            -3535354345111.5696876565435432,
            f64::MAX,
            f64::MAX / 10.0,
            f64::MIN * 10.0,
            f64::NAN,
        ] {
            let v = from_f64(n);
            let (lhs, rhs) = (n, to_f64(v));
            if lhs.is_nan() {
                assert!(rhs.is_nan());
            } else {
                assert_eq!(n, to_f64(v));
            }
        }
    }

    #[test]
    fn float_test2() {
        let mut gen = Codegen::new(false, Value::nil());

        let panic = gen.entry_panic;
        let assume_float_to_f64 = gen.jit.label();
        monoasm!(&mut gen.jit,
        assume_float_to_f64:
            pushq rbp;
        );
        gen.gen_val_to_f64_assume_float(0, panic);
        monoasm!(&mut gen.jit,
            popq rbp;
            ret;
        );
        let assume_int_to_f64 = gen.jit.label();
        monoasm!(&mut gen.jit,
        assume_int_to_f64:
            pushq rbp;
        );
        gen.gen_val_to_f64_assume_integer(0, panic);
        monoasm!(&mut gen.jit,
            popq rbp;
            ret;
        );
        gen.jit.finalize();
        let float_to_f64_entry = gen.jit.get_label_address(assume_float_to_f64);
        let int_to_f64_entry = gen.jit.get_label_address(assume_int_to_f64);

        let float_to_f64: fn(Value) -> f64 =
            unsafe { std::mem::transmute(float_to_f64_entry.as_ptr()) };
        let int_to_f64: fn(Value) -> f64 =
            unsafe { std::mem::transmute(int_to_f64_entry.as_ptr()) };
        assert_eq!(3.574, float_to_f64(Value::new_float(3.574)));
        assert_eq!(0.0, float_to_f64(Value::new_float(0.0)));
        assert_eq!(143.0, int_to_f64(Value::new_integer(143)));
        assert_eq!(14354813558.0, int_to_f64(Value::new_integer(14354813558)));
        assert_eq!(-143.0, int_to_f64(Value::new_integer(-143)));
    }

    #[test]
    fn float_test3() {
        let mut gen = Codegen::new(false, Value::nil());

        let panic = gen.entry_panic;
        let to_f64 = gen.jit.label();
        monoasm!(&mut gen.jit,
        to_f64:
            pushq rbp;
        );
        gen.gen_val_to_f64(0, panic);
        monoasm!(&mut gen.jit,
            popq rbp;
            ret;
        );
        gen.jit.finalize();
        let to_f64_entry = gen.jit.get_label_address(to_f64);

        let to_f64: fn(Value) -> f64 = unsafe { std::mem::transmute(to_f64_entry.as_ptr()) };
        assert_eq!(3.574, to_f64(Value::new_float(3.574)));
        assert_eq!(0.0, to_f64(Value::new_float(0.0)));
        assert_eq!(f64::MAX, to_f64(Value::new_float(f64::MAX)));
        assert_eq!(f64::MIN, to_f64(Value::new_float(f64::MIN)));
        assert!(to_f64(Value::new_float(f64::NAN)).is_nan());
        assert!(to_f64(Value::new_float(f64::INFINITY)).is_infinite());
        assert_eq!(143.0, to_f64(Value::new_integer(143)));
        assert_eq!(14354813558.0, to_f64(Value::new_integer(14354813558)));
        assert_eq!(-143.0, to_f64(Value::new_integer(-143)));
    }
}
