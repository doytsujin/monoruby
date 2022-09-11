use super::*;

///
/// kinds of binary operation.
///
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum BinOpK {
    Add = 0,
    Sub = 1,
    Mul = 2,
    Div = 3,
    BitOr = 4,
    BitAnd = 5,
    BitXor = 6,
    Shr = 7,
    Shl = 8,
}

use std::fmt;
impl fmt::Display for BinOpK {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let s = match *self {
            BinOpK::Add => "+",
            BinOpK::Sub => "-",
            BinOpK::Mul => "*",
            BinOpK::Div => "/",
            BinOpK::BitOr => "|",
            BinOpK::BitAnd => "&",
            BinOpK::BitXor => "^",
            BinOpK::Shr => ">>",
            BinOpK::Shl => "<<",
        };
        write!(f, "{}", s)
    }
}

impl BinOpK {
    fn from(i: u16) -> Self {
        match i {
            0 => BinOpK::Add,
            1 => BinOpK::Sub,
            2 => BinOpK::Mul,
            3 => BinOpK::Div,
            4 => BinOpK::BitOr,
            5 => BinOpK::BitAnd,
            6 => BinOpK::BitXor,
            7 => BinOpK::Shr,
            8 => BinOpK::Shl,
            _ => unreachable!(),
        }
    }

    pub fn generic_func(
        &self,
    ) -> extern "C" fn(&mut Interp, &mut Globals, Value, Value) -> Option<Value> {
        match self {
            BinOpK::Add => add_values,
            BinOpK::Sub => sub_values,
            BinOpK::Mul => mul_values,
            BinOpK::Div => div_values,
            BinOpK::BitOr => bitor_values,
            BinOpK::BitAnd => bitand_values,
            BinOpK::BitXor => bitxor_values,
            BinOpK::Shr => shr_values,
            BinOpK::Shl => shl_values,
        }
    }
}

///
/// bytecode Ir.
///
#[derive(Debug, Clone, PartialEq)]
pub(super) enum BcIr {
    Br(usize),
    CondBr(BcReg, usize, bool, BrKind),
    Integer(BcReg, i32),
    Symbol(BcReg, IdentId),
    Literal(BcReg, Value),
    Array(BcReg, BcReg, u16),
    Index(BcReg, BcReg, BcReg),       // ret, base, index
    IndexAssign(BcReg, BcReg, BcReg), // src, base, index
    LoadConst(BcReg, IdentId),
    StoreConst(BcReg, IdentId),
    LoadIvar(BcReg, IdentId),  // ret, id  - %ret = @id
    StoreIvar(BcReg, IdentId), // src, id  - @id = %src
    Nil(BcReg),
    Neg(BcReg, BcReg),                       // ret, src
    BinOp(BinOpK, BcReg, BcReg, BcReg),      // kind, ret, lhs, rhs
    BinOpRi(BinOpK, BcReg, BcReg, i16),      // kind, ret, lhs, rhs
    BinOpIr(BinOpK, BcReg, i16, BcReg),      // kind, ret, lhs, rhs
    Cmp(CmpKind, BcReg, BcReg, BcReg, bool), // kind, dst, lhs, rhs, optimizable
    Cmpri(CmpKind, BcReg, BcReg, i16, bool), // kind, dst, lhs, rhs, optimizable
    Ret(BcReg),
    Mov(BcReg, BcReg),                  // dst, offset
    MethodCall(Option<BcReg>, IdentId), // (ret, id)
    MethodArgs(BcReg, BcTemp, usize),   // (recv, args, args_len)
    InlineCache,
    MethodDef(IdentId, FuncId),
    ConcatStr(Option<BcReg>, BcTemp, usize), // (ret, args, args_len)
    LoopStart,
    LoopEnd,
}

#[derive(Clone, Copy, PartialEq)]
#[repr(C)]
pub struct Bc {
    pub(crate) op1: u64,
    pub(crate) op2: Bc2,
}

impl Bc {
    pub(crate) fn from(op1: u64) -> Self {
        Self {
            op1,
            op2: Bc2::from(0),
        }
    }

    pub(crate) fn from_with_value(op1: u64, val: Value) -> Self {
        Self {
            op1,
            op2: Bc2::from(val.get()),
        }
    }

    pub(crate) fn from_with_func_name_id(op1: u64, name: IdentId, func_id: FuncId) -> Self {
        Self {
            op1,
            op2: Bc2::from(((func_id.0 as u64) << 32) + (name.get() as u64)),
        }
    }

    pub(crate) fn from_with_class_and_version(op1: u64, class_id: ClassId, version: u32) -> Self {
        Self {
            op1,
            op2: Bc2::class_and_version(class_id, version),
        }
    }

    pub(crate) fn from_with_class2(op1: u64) -> Self {
        Self {
            op1,
            op2: Bc2::class2(INTEGER_CLASS, INTEGER_CLASS),
        }
    }

    pub(crate) fn classid1(&self) -> ClassId {
        ClassId::new(self.op2.0 as u32)
    }

    pub(crate) fn classid2(&self) -> ClassId {
        ClassId::new((self.op2.0 >> 32) as u32)
    }

    pub(crate) fn value(&self) -> Option<Value> {
        match self.op2.0 {
            0 => None,
            v => Some(Value::from(v)),
        }
    }

    pub(crate) fn from_jit_addr(&self) -> u64 {
        self.op2.0
    }

    pub(crate) fn is_integer1(&self) -> bool {
        self.classid1() == INTEGER_CLASS
    }

    pub(crate) fn is_integer2(&self) -> bool {
        self.classid2() == INTEGER_CLASS
    }

    pub(crate) fn is_float1(&self) -> bool {
        self.classid1() == FLOAT_CLASS
    }

    pub(crate) fn is_float2(&self) -> bool {
        self.classid2() == FLOAT_CLASS
    }

    pub(crate) fn is_binary_integer(&self) -> bool {
        self.classid1() == INTEGER_CLASS && self.classid2() == INTEGER_CLASS
    }

    pub(crate) fn is_binary_float(&self) -> bool {
        match (self.classid1(), self.classid2()) {
            (INTEGER_CLASS, INTEGER_CLASS) => false,
            (INTEGER_CLASS | FLOAT_CLASS, INTEGER_CLASS | FLOAT_CLASS) => true,
            _ => false,
        }
    }
}

impl std::fmt::Debug for Bc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        fn optstr(opt: bool) -> &'static str {
            if opt {
                "_"
            } else {
                ""
            }
        }
        fn disp_str(disp: i32) -> String {
            if disp >= 0 {
                format!("+{:05}", disp + 1)
            } else {
                format!("{:05}", disp + 1)
            }
        }
        match BcOp::from_bc(self) {
            BcOp::Br(disp) => {
                write!(f, "br => {}", disp_str(disp))
            }
            BcOp::CondBr(reg, disp, opt, kind) => {
                write!(
                    f,
                    "cond{}br {}{:?} => {}",
                    kind.to_s(),
                    optstr(opt),
                    reg,
                    disp_str(disp)
                )
            }
            BcOp::Integer(reg, num) => write!(f, "{:?} = {}: i32", reg, num),
            BcOp::Symbol(reg, id) => {
                write!(f, "{:?} = {:?}", reg, id)
            }
            BcOp::Literal(reg, id) => {
                write!(f, "{:?} = literal[#{:?}]", reg, id)
            }
            BcOp::Array(ret, src, len) => {
                write!(f, "{:?} = array[{:?}; {}]", ret, src, len)
            }
            BcOp::Index(ret, base, idx) => {
                write!(f, "{:?} = {:?}.[{:?}]", ret, base, idx)
            }
            BcOp::IndexAssign(src, base, idx) => {
                write!(f, "{:?}.[{:?}] = {:?}", base, idx, src)
            }
            BcOp::LoadConst(reg, id) => {
                write!(f, "{:?} = const[{:?}]", reg, id)
            }
            BcOp::StoreConst(reg, id) => {
                write!(f, "const[{:?}] = {:?}", id, reg)
            }
            BcOp::LoadIvar(reg, id) => {
                write!(f, "{:?} = @[{:?}]", reg, id)
            }
            BcOp::StoreIvar(reg, id) => {
                write!(f, "@[{:?}] = {:?}", id, reg)
            }
            BcOp::Nil(reg) => write!(f, "{:?} = nil", reg),
            BcOp::Neg(dst, src) => write!(f, "{:?} = neg {:?}", dst, src),
            BcOp::BinOp(kind, dst, lhs, rhs) => {
                let class_id = self.classid1();
                let class_id2 = self.classid2();
                let op1 = format!("{:?} = {:?} {} {:?}", dst, lhs, kind, rhs);
                write!(f, "{:28} [{:?}][{:?}]", op1, class_id, class_id2)
            }
            BcOp::BinOpRi(kind, dst, lhs, rhs) => {
                let class_id = self.classid1();
                let class_id2 = self.classid2();
                let op1 = format!("{:?} = {:?} {} {}: i16", dst, lhs, kind, rhs);
                write!(f, "{:28} [{:?}][{:?}]", op1, class_id, class_id2)
            }
            BcOp::BinOpIr(kind, dst, lhs, rhs) => {
                let class_id = self.classid1();
                let class_id2 = self.classid2();
                let op1 = format!("{:?} = {}: i16 {} {:?}", dst, lhs, kind, rhs);
                write!(f, "{:28} [{:?}][{:?}]", op1, class_id, class_id2)
            }
            BcOp::Cmp(kind, dst, lhs, rhs, opt) => {
                let class_id = self.classid1();
                let class_id2 = self.classid2();
                let op1 = format!("{}{:?} = {:?} {:?} {:?}", optstr(opt), dst, lhs, kind, rhs,);
                write!(f, "{:28} [{:?}][{:?}]", op1, class_id, class_id2)
            }
            BcOp::Cmpri(kind, dst, lhs, rhs, opt) => {
                let class_id = self.classid1();
                let class_id2 = self.classid2();
                let op1 = format!(
                    "{}{:?} = {:?} {:?} {}: i16",
                    optstr(opt),
                    dst,
                    lhs,
                    kind,
                    rhs,
                );
                write!(f, "{:28} [{:?}][{:?}]", op1, class_id, class_id2)
            }

            BcOp::Ret(reg) => write!(f, "ret {:?}", reg),
            BcOp::Mov(dst, src) => write!(f, "{:?} = {:?}", dst, src),
            BcOp::MethodCall(ret, name) => {
                let class_id = self.classid1();
                let op1 = format!("{} = call {:?}", ret.ret_str(), name,);
                write!(f, "{:28} {:?}", op1, class_id)
            }
            BcOp::MethodArgs(recv, args, len) => {
                write!(f, "{:?}.call_args ({:?}; {})", recv, args, len)
            }
            BcOp::MethodDef(name, _) => {
                write!(f, "define {:?}", name)
            }
            BcOp::ConcatStr(ret, args, len) => {
                write!(f, "{} = concat({:?}; {})", ret.ret_str(), args, len)
            }
            BcOp::LoopStart(count) => writeln!(
                f,
                "loop_start counter={} jit-addr={:016x}",
                count,
                self.from_jit_addr()
            ),
            BcOp::LoopEnd => write!(f, "loop_end"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(transparent)]
pub(crate) struct Bc2(u64);

impl Bc2 {
    pub(crate) fn from(op: u64) -> Self {
        Self(op)
    }

    fn class_and_version(class_id: ClassId, version: u32) -> Self {
        let id: u32 = class_id.into();
        Self(((version as u64) << 32) + (id as u64))
    }

    fn class2(class_id1: ClassId, class_id2: ClassId) -> Self {
        let id1: u32 = class_id1.into();
        let id2: u32 = class_id2.into();
        Self(((id2 as u64) << 32) + (id1 as u64))
    }

    fn get_value(&self) -> Value {
        Value::from(self.0)
    }
}

///
/// Bytecode instructions.
///
#[derive(Debug, Clone, PartialEq)]
pub(super) enum BcOp {
    /// branch(dest)
    Br(i32),
    /// conditional branch(%reg, dest, optimizable)  : branch when reg was true.
    CondBr(SlotId, i32, bool, BrKind),
    /// integer(%reg, i32)
    Integer(SlotId, i32),
    /// Symbol(%reg, IdentId)
    Symbol(SlotId, IdentId),
    /// literal(%ret, value)
    Literal(SlotId, Value),
    /// array(%ret, %src, len)
    Array(SlotId, SlotId, u16),
    /// index(%ret, %base, %idx)
    Index(SlotId, SlotId, SlotId),
    /// index(%src, %base, %idx)
    IndexAssign(SlotId, SlotId, SlotId),
    LoadConst(SlotId, ConstSiteId),
    StoreConst(SlotId, IdentId),
    LoadIvar(SlotId, IdentId),  // ret, id  - %ret = @id
    StoreIvar(SlotId, IdentId), // src, id  - @id = %src
    /// nil(%reg)
    Nil(SlotId),
    /// negate(%ret, %src)
    Neg(SlotId, SlotId),
    /// binop(kind, %ret, %lhs, %rhs)
    BinOp(BinOpK, SlotId, SlotId, SlotId),
    /// binop with small integer(kind, %ret, %lhs, rhs)
    BinOpRi(BinOpK, SlotId, SlotId, i16),
    /// binop with small integer(kind, %ret, lhs, %rhs)
    BinOpIr(BinOpK, SlotId, i16, SlotId),
    /// cmp(%ret, %lhs, %rhs, optimizable)
    Cmp(CmpKind, SlotId, SlotId, SlotId, bool),
    /// cmpri(%ret, %lhs, rhs: i16, optimizable)
    Cmpri(CmpKind, SlotId, SlotId, i16, bool),
    /// return(%ret)
    Ret(SlotId),
    /// move(%dst, %src)
    Mov(SlotId, SlotId),
    /// func call(%ret, name)
    MethodCall(SlotId, IdentId),
    /// func call 2nd opecode(%recv, %args, len)
    MethodArgs(SlotId, SlotId, u16),
    /// method definition(method_name, func_id)
    MethodDef(IdentId, FuncId),
    /// concatenate strings(ret, args, args_len)
    ConcatStr(SlotId, SlotId, u16),
    /// loop start marker
    LoopStart(u32),
    LoopEnd,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum BrKind {
    BrIf = 0,
    BrIfNot = 1,
}

impl BrKind {
    pub(super) fn from(i: u16) -> Self {
        match i {
            0 => Self::BrIf,
            1 => Self::BrIfNot,
            _ => unreachable!(),
        }
    }

    pub(super) fn to_s(&self) -> &'static str {
        match self {
            Self::BrIf => "",
            Self::BrIfNot => "not",
        }
    }
}

fn dec_wl(op: u64) -> (u16, u32) {
    ((op >> 32) as u16, op as u32)
}

fn dec_www(op: u64) -> (u16, u16, u16) {
    ((op >> 32) as u16, (op >> 16) as u16, op as u16)
}

impl BcOp {
    pub fn from_bc(bcop: &Bc) -> Self {
        let op = bcop.op1;
        let opcode = (op >> 48) as u16;
        if opcode & 0x80 == 0 {
            let (op1, op2) = dec_wl(op);
            match opcode {
                1 => Self::MethodCall(SlotId::new(op1), IdentId::from(op2)),
                2 => Self::MethodDef(
                    IdentId::from((bcop.op2.0) as u32),
                    FuncId((bcop.op2.0 >> 32) as u32),
                ),
                3 => Self::Br(op2 as i32),
                4 => Self::CondBr(SlotId::new(op1), op2 as i32, false, BrKind::BrIf),
                5 => Self::CondBr(SlotId::new(op1), op2 as i32, false, BrKind::BrIfNot),
                6 => Self::Integer(SlotId::new(op1), op2 as i32),
                7 => Self::Literal(SlotId::new(op1), bcop.op2.get_value()),
                8 => Self::Nil(SlotId::new(op1)),
                9 => Self::Symbol(SlotId::new(op1), IdentId::from(op2)),
                10 => Self::LoadConst(SlotId::new(op1), ConstSiteId(op2)),
                11 => Self::StoreConst(SlotId::new(op1), IdentId::from(op2)),
                12..=13 => Self::CondBr(
                    SlotId::new(op1),
                    op2 as i32,
                    true,
                    BrKind::from(opcode - 12),
                ),
                14 => Self::LoopStart(op2),
                15 => Self::LoopEnd,
                16 => Self::LoadIvar(SlotId::new(op1), IdentId::from(op2)),
                17 => Self::StoreIvar(SlotId::new(op1), IdentId::from(op2)),
                _ => unreachable!("{:016x}", op),
            }
        } else {
            let (op1, op2, op3) = dec_www(op);
            match opcode {
                129 => Self::Neg(SlotId::new(op1), SlotId::new(op2)),
                130 => Self::MethodArgs(SlotId::new(op1), SlotId::new(op2), op3),
                131 => Self::Array(SlotId::new(op1), SlotId::new(op2), op3),
                132 => Self::Index(SlotId::new(op1), SlotId::new(op2), SlotId::new(op3)),
                133 => Self::IndexAssign(SlotId::new(op1), SlotId::new(op2), SlotId::new(op3)),
                134..=139 => Self::Cmp(
                    CmpKind::from(opcode - 134),
                    SlotId::new(op1),
                    SlotId::new(op2),
                    SlotId::new(op3),
                    false,
                ),
                142..=147 => Self::Cmpri(
                    CmpKind::from(opcode - 142),
                    SlotId::new(op1),
                    SlotId::new(op2),
                    op3 as i16,
                    false,
                ),
                148 => Self::Ret(SlotId::new(op1)),
                149 => Self::Mov(SlotId::new(op1), SlotId::new(op2)),
                155 => Self::ConcatStr(SlotId::new(op1), SlotId::new(op2), op3),
                156..=161 => Self::Cmp(
                    CmpKind::from(opcode - 156),
                    SlotId(op1),
                    SlotId(op2),
                    SlotId(op3),
                    true,
                ),
                162..=167 => Self::Cmpri(
                    CmpKind::from(opcode - 162),
                    SlotId::new(op1),
                    SlotId::new(op2),
                    op3 as i16,
                    true,
                ),
                170..=178 => Self::BinOp(
                    BinOpK::from(opcode - 170),
                    SlotId::new(op1),
                    SlotId::new(op2),
                    SlotId::new(op3),
                ),
                180..=188 => Self::BinOpIr(
                    BinOpK::from(opcode - 180),
                    SlotId::new(op1),
                    op2 as i16,
                    SlotId::new(op3),
                ),
                190..=198 => Self::BinOpRi(
                    BinOpK::from(opcode - 190),
                    SlotId::new(op1),
                    SlotId::new(op2),
                    op3 as i16,
                ),
                _ => unreachable!("{:016x}", op),
            }
        }
    }
}
