use std::collections::{BTreeSet, HashMap};

use super::parse::Span;
use super::*;

///
/// A state of HIR.
///
#[derive(Clone, PartialEq)]
pub struct HIRContext {
    /// SSA register information.
    reginfo: Vec<SsaRegInfo>,
    /// Basic blocks.
    pub basic_block: Vec<HirBasicBlock>,
    cur_bb: usize,
    /// Functions.
    pub functions: Vec<HirFunction>,
    cur_fn: usize,
}

impl std::fmt::Debug for HIRContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "HIRContxt {{")?;

        for func in &self.functions {
            writeln!(f, "\tFunction {} {{", func.name)?;
            for i in func.bbs.iter() {
                let bb = &self.basic_block[*i];
                writeln!(f, "\t\tBasicBlock {} {{ owner:{:?}", i, bb.owner_function)?;
                for hir in &bb.insts {
                    let s = match hir {
                        Hir::Integer(ret, i) => {
                            format!("%{}: {:?} = {}: i32", ret, self[*ret].ty, i)
                        }
                        Hir::Float(ret, f) => format!("%{}: {:?} = {}: f64", ret, self[*ret].ty, f),
                        Hir::CastIntFloat(op) => {
                            format!(
                                "%{}: {:?} = cast {:?} i32 to f64",
                                op.ret, self[op.ret].ty, op.src
                            )
                        }
                        Hir::INeg(op) => {
                            format!("%{}: {:?} = ineg {:?}", op.ret, self[op.ret].ty, op.src)
                        }
                        Hir::FNeg(op) => {
                            format!("%{}: {:?} = fneg {:?}", op.ret, self[op.ret].ty, op.src)
                        }
                        Hir::IAdd(op) => format!(
                            "%{}: {:?} = iadd {:?}, {:?}",
                            op.ret, self[op.ret].ty, op.lhs, op.rhs
                        ),
                        Hir::FAdd(op) => format!(
                            "%{}: {:?} = fadd {:?}, {:?}",
                            op.ret, self[op.ret].ty, op.lhs, op.rhs
                        ),
                        Hir::ISub(op) => format!(
                            "%{}: {:?} = isub {:?}, {:?}",
                            op.ret, self[op.ret].ty, op.lhs, op.rhs
                        ),
                        Hir::FSub(op) => format!(
                            "%{}: {:?} = fsub {:?}, {:?}",
                            op.ret, self[op.ret].ty, op.lhs, op.rhs
                        ),
                        Hir::IMul(op) => format!(
                            "%{}: {:?} = imul %{}, %{}",
                            op.ret, self[op.ret].ty, op.lhs, op.rhs
                        ),
                        Hir::FMul(op) => format!(
                            "%{}: {:?} = fmul {:?}, {:?}",
                            op.ret, self[op.ret].ty, op.lhs, op.rhs
                        ),
                        Hir::IDiv(op) => format!(
                            "%{}: {:?} = idiv %{}, %{}",
                            op.ret, self[op.ret].ty, op.lhs, op.rhs
                        ),
                        Hir::FDiv(op) => format!(
                            "%{}: {:?} = fdiv {:?}, {:?}",
                            op.ret, self[op.ret].ty, op.lhs, op.rhs
                        ),
                        Hir::ICmp(kind, op) => format!(
                            "%{}: {:?} = icmp {:?} {:?}, {:?}",
                            op.ret, self[op.ret].ty, kind, op.lhs, op.rhs
                        ),
                        Hir::FCmp(kind, op) => format!(
                            "%{}: {:?} = fcmp {:?} {:?}, {:?}",
                            op.ret, self[op.ret].ty, kind, op.lhs, op.rhs
                        ),
                        Hir::Ret(ret) => format!("ret {:?}", ret),
                        Hir::LocalStore(ret, ident, rhs) => {
                            if let Some(ret) = ret {
                                format!("${}: {:?} | %{} = %{}", ident.0, ident.1, ret, rhs)
                            } else {
                                format!("${}: {:?} = %{}", ident.0, ident.1, rhs)
                            }
                        }
                        Hir::LocalLoad(ident, lhs) => {
                            format!("%{} = ${}: {:?}", lhs, ident.0, ident.1)
                        }
                        Hir::Br(dest) => format!("br {}", dest),
                        Hir::ICmpBr(kind, lhs, rhs, then_, else_) => {
                            format!(
                                "cmpbr ({:?} %{}, {:?}) then {} else {}",
                                kind, lhs, rhs, then_, else_
                            )
                        }
                        Hir::FCmpBr(kind, lhs, rhs, then_, else_) => {
                            format!(
                                "cmpbr ({:?} %{}, %{}) then {} else {}",
                                kind, lhs, rhs, then_, else_
                            )
                        }
                        Hir::CondBr(cond, then_, else_) => {
                            format!("condbr %{} then {} else {}", cond, then_, else_)
                        }
                        Hir::Phi(ret, phi) => {
                            let phi_s = phi
                                .iter()
                                .map(|(bb, r)| format!("({},%{})", bb, r))
                                .collect::<Vec<String>>()
                                .join(", ");
                            format!("%{} = phi {}", ret, phi_s)
                        }
                    };
                    writeln!(f, "\t\t\t{}", s)?;
                }
                writeln!(f, "\t\t}}")?;
            }
            writeln!(f, "\t}}")?;
        }
        writeln!(f, "}}")
    }
}

impl std::ops::Deref for HIRContext {
    type Target = HirBasicBlock;

    fn deref(&self) -> &Self::Target {
        &self.basic_block[self.cur_bb]
    }
}

impl std::ops::DerefMut for HIRContext {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.basic_block[self.cur_bb]
    }
}

impl std::ops::Index<SsaReg> for HIRContext {
    type Output = SsaRegInfo;

    fn index(&self, i: SsaReg) -> &SsaRegInfo {
        &self.reginfo[i.to_usize()]
    }
}

impl std::ops::IndexMut<SsaReg> for HIRContext {
    fn index_mut(&mut self, i: SsaReg) -> &mut SsaRegInfo {
        &mut self.reginfo[i.to_usize()]
    }
}

impl std::ops::Index<usize> for HIRContext {
    type Output = HirBasicBlock;

    fn index(&self, i: usize) -> &HirBasicBlock {
        &self.basic_block[i]
    }
}

impl std::ops::IndexMut<usize> for HIRContext {
    fn index_mut(&mut self, i: usize) -> &mut HirBasicBlock {
        &mut self.basic_block[i]
    }
}

impl HIRContext {
    pub fn new() -> Self {
        let cur_bb = 0;
        let cur_fn = 0;
        let basic_block = HirBasicBlock::new(cur_fn);
        let mut function = HirFunction::new("/main".to_string(), cur_bb);
        function.bbs.insert(cur_bb);
        HIRContext {
            reginfo: vec![],
            basic_block: vec![basic_block],
            cur_bb,
            functions: vec![function],
            cur_fn,
        }
    }

    fn new_bb(&mut self) -> usize {
        let bb = HirBasicBlock::new(self.cur_fn);
        let next = self.basic_block.len();
        self.functions[self.cur_fn].bbs.insert(next);
        self.basic_block.push(bb);
        next
    }

    fn enter_new_func(&mut self, name: String) -> usize {
        let entry_bb = self.basic_block.len();
        let next_fn = self.functions.len();

        let bb = HirBasicBlock::new(next_fn);
        self.basic_block.push(bb);

        let mut func = HirFunction::new(name, entry_bb);
        func.bbs.insert(entry_bb);
        self.functions.push(func);

        self.cur_fn = next_fn;
        self.cur_bb = entry_bb;
        next_fn
    }

    fn add_assign(&mut self, hir: Hir, ty: Type) -> SsaReg {
        let ret_reg = self.next_reg();
        self.reginfo.push(SsaRegInfo::new(ty));
        self.insts.push(hir);
        ret_reg
    }

    pub fn register_num(&self) -> usize {
        self.reginfo.len()
    }

    fn next_reg(&self) -> SsaReg {
        SsaReg(self.reginfo.len())
    }

    fn new_integer(&mut self, i: i32) -> SsaReg {
        self.add_assign(Hir::Integer(self.next_reg(), i), Type::Integer)
    }

    fn new_float(&mut self, f: f64) -> SsaReg {
        self.add_assign(Hir::Float(self.next_reg(), f), Type::Float)
    }

    fn new_as_float(&mut self, src: SsaReg) -> SsaReg {
        let ret = self.next_reg();
        self.add_assign(
            Hir::CastIntFloat(HirUnop {
                ret,
                src: HirOperand::Reg(src),
            }),
            Type::Float,
        )
    }

    fn new_as_float_imm(&mut self, src: i32) -> SsaReg {
        let ret = self.next_reg();
        self.add_assign(
            Hir::CastIntFloat(HirUnop {
                ret,
                src: HirOperand::Const(Value::Integer(src)),
            }),
            Type::Float,
        )
    }

    fn new_ineg(&mut self, src: SsaReg) -> SsaReg {
        let ret = self.next_reg();
        self.add_assign(
            Hir::INeg(HirUnop {
                ret,
                src: HirOperand::Reg(src),
            }),
            Type::Integer,
        )
    }

    fn new_fneg(&mut self, src: SsaReg) -> SsaReg {
        let ret = self.next_reg();
        self.add_assign(
            Hir::FNeg(HirUnop {
                ret,
                src: HirOperand::Reg(src),
            }),
            Type::Float,
        )
    }

    fn new_iadd(&mut self, lhs: HirOperand, rhs: HirOperand) -> SsaReg {
        let ret = self.next_reg();
        self.add_assign(Hir::IAdd(HirBinop2 { ret, lhs, rhs }), Type::Integer)
    }

    fn new_fadd(&mut self, lhs: HirOperand, rhs: HirOperand) -> SsaReg {
        let ret = self.next_reg();
        self.add_assign(Hir::FAdd(HirBinop2 { ret, lhs, rhs }), Type::Float)
    }

    fn new_isub(&mut self, lhs: HirOperand, rhs: HirOperand) -> SsaReg {
        let ret = self.next_reg();
        self.add_assign(Hir::ISub(HirBinop2 { ret, lhs, rhs }), Type::Integer)
    }

    fn new_fsub(&mut self, lhs: HirOperand, rhs: HirOperand) -> SsaReg {
        let ret = self.next_reg();
        self.add_assign(Hir::FSub(HirBinop2 { ret, lhs, rhs }), Type::Float)
    }

    fn new_imul(&mut self, lhs: SsaReg, rhs: SsaReg) -> SsaReg {
        let ret = self.next_reg();
        self.add_assign(Hir::IMul(HIRBinop { ret, lhs, rhs }), Type::Integer)
    }

    fn new_fmul(&mut self, lhs: HirOperand, rhs: HirOperand) -> SsaReg {
        let ret = self.next_reg();
        self.add_assign(Hir::FMul(HirBinop2 { ret, lhs, rhs }), Type::Float)
    }

    fn new_idiv(&mut self, lhs: SsaReg, rhs: SsaReg) -> SsaReg {
        let ret = self.next_reg();
        self.add_assign(Hir::IDiv(HIRBinop { ret, lhs, rhs }), Type::Integer)
    }

    fn new_fdiv(&mut self, lhs: HirOperand, rhs: HirOperand) -> SsaReg {
        let ret = self.next_reg();
        self.add_assign(Hir::FDiv(HirBinop2 { ret, lhs, rhs }), Type::Float)
    }

    fn new_icmp(&mut self, kind: CmpKind, lhs: HirOperand, rhs: HirOperand) -> SsaReg {
        let ret = self.next_reg();
        self.add_assign(Hir::ICmp(kind, HirBinop2 { ret, lhs, rhs }), Type::Bool)
    }

    fn new_fcmp(&mut self, kind: CmpKind, lhs: SsaReg, rhs: SsaReg) -> SsaReg {
        let ret = self.next_reg();
        self.add_assign(Hir::FCmp(kind, HIRBinop { ret, lhs, rhs }), Type::Bool)
    }

    fn new_ret(&mut self, lhs: SsaReg) {
        let hir = Hir::Ret(HirOperand::Reg(lhs));
        self.insts.push(hir);
    }

    fn new_local_store(
        &mut self,
        local_map: &mut HashMap<String, (usize, Type)>,
        ident: &String,
        rhs: SsaReg,
    ) -> Result<SsaReg> {
        let ty = self[rhs].ty;
        let len = local_map.len();
        let info = match local_map.get(ident) {
            Some(info) => info.clone(),
            None => {
                let info = (len, ty);
                local_map.insert(ident.to_string(), info.clone());
                info
            }
        };
        if info.1 != ty {
            return Err(HirErr::TypeMismatch(info.1, ty));
        }
        let ret = self.next_reg();
        self.add_assign(Hir::LocalStore(Some(ret), info, rhs), ty);
        Ok(ret)
    }

    fn new_local_store_nouse(
        &mut self,
        local_map: &mut HashMap<String, (usize, Type)>,
        ident: &String,
        rhs: SsaReg,
    ) -> Result<()> {
        let ty = self[rhs].ty;
        let len = local_map.len();
        let info = match local_map.get(ident) {
            Some(info) => info.clone(),
            None => {
                let info = (len, ty);
                local_map.insert(ident.to_string(), info.clone());
                info
            }
        };
        if info.1 != ty {
            return Err(HirErr::TypeMismatch(info.1, ty));
        }
        let hir = Hir::LocalStore(None, info, rhs);
        self.insts.push(hir);
        Ok(())
    }

    fn new_local_load(
        &mut self,
        local_map: &mut HashMap<String, (usize, Type)>,
        ident: &String,
    ) -> Result<SsaReg> {
        let info = match local_map.get(ident) {
            Some(info) => info.clone(),
            None => return Err(HirErr::UndefinedLocal(ident.clone())),
        };
        let ty = info.1;
        let hir = Hir::LocalLoad(info, self.next_reg());
        Ok(self.add_assign(hir, ty))
    }

    fn new_phi(&mut self, phi: Vec<(usize, SsaReg)>) -> SsaReg {
        let ty = self[phi[0].1].ty;
        assert!(phi.iter().all(|(_, r)| self[*r].ty == ty));
        let ret = self.next_reg();
        self.add_assign(Hir::Phi(ret, phi), ty)
    }
}

#[derive(Clone, PartialEq)]
pub struct HirFunction {
    pub name: String,
    pub entry_bb: usize,
    pub ret: Option<SsaReg>,
    pub ret_ty: Option<Type>,
    pub register_num: usize,
    pub bbs: BTreeSet<usize>,
}

impl HirFunction {
    fn new(name: String, entry_bb: usize) -> Self {
        Self {
            name,
            entry_bb,
            ret: None,
            ret_ty: None,
            register_num: 0,
            bbs: BTreeSet::default(),
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct HirBasicBlock {
    /// HIR instructions.
    pub insts: Vec<Hir>,
    /// The function this bb is owned.
    pub owner_function: usize,
}

impl HirBasicBlock {
    fn new(owner_function: usize) -> Self {
        Self {
            insts: vec![],
            owner_function,
        }
    }
}

#[derive(Debug, Clone)]
pub enum HirErr {
    UndefinedLocal(String),
    TypeMismatch(Type, Type),
}

type Result<T> = std::result::Result<T, HirErr>;

///
/// Instructions of High-level IR.
///
#[derive(Clone, Debug, PartialEq)]
pub enum Hir {
    Br(usize),
    CondBr(SsaReg, usize, usize),
    ICmpBr(CmpKind, SsaReg, HirOperand, usize, usize),
    FCmpBr(CmpKind, SsaReg, SsaReg, usize, usize),
    Phi(SsaReg, Vec<(usize, SsaReg)>),
    Integer(SsaReg, i32),
    Float(SsaReg, f64),
    CastIntFloat(HirUnop),
    INeg(HirUnop),
    FNeg(HirUnop),
    IAdd(HirBinop2),
    ISub(HirBinop2),
    IMul(HIRBinop),
    IDiv(HIRBinop),
    FAdd(HirBinop2),
    FSub(HirBinop2),
    FMul(HirBinop2),
    FDiv(HirBinop2),
    ICmp(CmpKind, HirBinop2),
    FCmp(CmpKind, HIRBinop),
    Ret(HirOperand),
    LocalStore(Option<SsaReg>, (usize, Type), SsaReg), // (ret, (offset, type), rhs)
    LocalLoad((usize, Type), SsaReg),
}

///
/// Binary operations.
///
#[derive(Clone, Debug, PartialEq)]
pub struct HIRBinop {
    /// Register ID of return value.
    pub ret: SsaReg,
    /// Register ID of left-hand side.
    pub lhs: SsaReg,
    /// Register ID of right-hand side.
    pub rhs: SsaReg,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HirBinop2 {
    /// Register ID of return value.
    pub ret: SsaReg,
    /// Register ID of left-hand side.
    pub lhs: HirOperand,
    /// Register ID of right-hand side.
    pub rhs: HirOperand,
}

///
/// Unary operations.
///
#[derive(Clone, Debug, PartialEq)]
pub struct HirUnop {
    /// Register ID of return value.
    pub ret: SsaReg,
    /// Register ID of source value.
    pub src: HirOperand,
}

#[derive(Clone, PartialEq)]
pub enum HirOperand {
    Reg(SsaReg),
    Const(Value),
}

impl std::fmt::Debug for HirOperand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reg(r) => write!(f, "%{}", r.to_usize()),
            Self::Const(c) => write!(f, "{:?}", c),
        }
    }
}

impl HirOperand {
    fn integer(n: i32) -> Self {
        Self::Const(Value::Integer(n))
    }

    fn float(n: f64) -> Self {
        Self::Const(Value::Float(n))
    }

    fn reg(r: SsaReg) -> Self {
        Self::Reg(r)
    }
}

///
/// ID of SSA registers.
///
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SsaReg(usize);

impl std::fmt::Display for SsaReg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl SsaReg {
    pub fn to_usize(self) -> usize {
        self.0
    }
}

///
/// Information of SSA registers.
///
#[derive(Clone, PartialEq)]
pub struct SsaRegInfo {
    /// *Type* of the register.
    pub ty: Type,
}

impl std::fmt::Debug for SsaRegInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.ty)
    }
}

impl SsaRegInfo {
    fn new(ty: Type) -> Self {
        Self { ty }
    }
}

macro_rules! binary_ops {
    ($self:ident, $map:ident, $lhs:ident, $rhs:ident, $i_op:ident, $f_op:ident) => {
        match (&$lhs.0, &$rhs.0) {
            (Expr::Integer(lhs_), Expr::Float(rhs_)) => {
                let lhs = $self.new_as_float_imm(*lhs_);
                Ok($self.$f_op(HirOperand::reg(lhs), HirOperand::float(*rhs_)))
            }
            (Expr::Integer(lhs_), Expr::Integer(rhs_)) => {
                Ok($self.$i_op(HirOperand::integer(*lhs_), HirOperand::integer(*rhs_)))
            }
            (Expr::Integer(lhs_), _) => {
                let rhs = $self.gen($map, &$rhs.0)?;
                let rhs_ty = $self[rhs].ty;
                match rhs_ty {
                    Type::Integer => {
                        Ok($self.$i_op(HirOperand::integer(*lhs_), HirOperand::reg(rhs)))
                    }
                    Type::Float => {
                        let lhs = $self.new_as_float_imm(*lhs_);
                        Ok($self.$f_op(HirOperand::Reg(lhs), HirOperand::Reg(rhs)))
                    }
                    ty => Err(HirErr::TypeMismatch(ty, rhs_ty)),
                }
            }
            (Expr::Float(lhs_), Expr::Integer(rhs_)) => {
                let rhs = $self.new_as_float_imm(*rhs_);
                Ok($self.$f_op(HirOperand::float(*lhs_), HirOperand::reg(rhs)))
            }
            (Expr::Float(lhs_), Expr::Float(rhs_)) => {
                Ok($self.$f_op(HirOperand::float(*lhs_), HirOperand::float(*rhs_)))
            }
            (Expr::Float(lhs_), _) => {
                let rhs = $self.gen($map, &$rhs.0)?;
                let rhs_ty = $self[rhs].ty;
                match rhs_ty {
                    Type::Integer => {
                        let rhs = $self.new_as_float(rhs);
                        Ok($self.$f_op(HirOperand::float(*lhs_), HirOperand::reg(rhs)))
                    }
                    Type::Float => Ok($self.$f_op(HirOperand::float(*lhs_), HirOperand::reg(rhs))),
                    ty => Err(HirErr::TypeMismatch(ty, rhs_ty)),
                }
            }
            (_, Expr::Integer(rhs_)) => {
                let lhs = $self.gen($map, &$lhs.0)?;
                let lhs_ty = $self[lhs].ty;
                match lhs_ty {
                    Type::Integer => {
                        Ok($self.$i_op(HirOperand::reg(lhs), HirOperand::integer(*rhs_)))
                    }
                    Type::Float => {
                        Ok($self.$f_op(HirOperand::reg(lhs), HirOperand::float(*rhs_ as f64)))
                    }
                    ty => Err(HirErr::TypeMismatch(ty, Type::Integer)),
                }
            }
            (_, Expr::Float(rhs_)) => {
                let lhs = $self.gen($map, &$lhs.0)?;
                let lhs_ty = $self[lhs].ty;
                match lhs_ty {
                    Type::Integer => {
                        let lhs = $self.new_as_float(lhs);
                        Ok($self.$f_op(HirOperand::reg(lhs), HirOperand::float(*rhs_)))
                    }
                    Type::Float => Ok($self.$f_op(HirOperand::reg(lhs), HirOperand::float(*rhs_))),
                    ty => Err(HirErr::TypeMismatch(ty, Type::Float)),
                }
            }
            _ => {
                let lhs = $self.gen($map, &$lhs.0)?;
                let rhs = $self.gen($map, &$rhs.0)?;
                let lhs_ty = $self[lhs].ty;
                let rhs_ty = $self[rhs].ty;
                match (lhs_ty, rhs_ty) {
                    (Type::Integer, Type::Integer) => {
                        Ok($self.$i_op(HirOperand::Reg(lhs), HirOperand::Reg(rhs)))
                    }
                    (Type::Integer, Type::Float) => {
                        let lhs = $self.new_as_float(lhs);
                        Ok($self.$f_op(HirOperand::Reg(lhs), HirOperand::Reg(rhs)))
                    }
                    (Type::Float, Type::Integer) => {
                        let rhs = $self.new_as_float(rhs);
                        Ok($self.$f_op(HirOperand::Reg(lhs), HirOperand::Reg(rhs)))
                    }
                    (Type::Float, Type::Float) => {
                        Ok($self.$f_op(HirOperand::Reg(lhs), HirOperand::Reg(rhs)))
                    }
                    (ty_l, ty_r) => Err(HirErr::TypeMismatch(ty_l, ty_r)),
                }
            }
        }
    };
}

impl HIRContext {
    /// Generate HIR in top level from [(Stmt, Span)].
    pub fn from_ast(
        &mut self,
        local_map: &mut HashMap<String, (usize, Type)>,
        ast: &[(Stmt, Span)],
    ) -> Result<(SsaReg, Type)> {
        assert_eq!(0, self.cur_fn);
        let len = ast.len();
        let ret = if len == 0 {
            self.new_integer(0)
        } else {
            self.gen_stmts(local_map, ast)?
        };
        let ty = self[ret].ty;
        self.functions[self.cur_fn].register_num = self.register_num();
        self.new_ret(ret);
        Ok((ret, ty))
    }

    /// Generate HIR in new function from [(Stmt, Span)].
    pub fn new_func_from_ast(
        &mut self,
        func_name: String,
        local_map: &mut HashMap<String, (usize, Type)>,
        ast: &[(Expr, Span)],
    ) -> Result<usize> {
        let save = (self.cur_fn, self.cur_bb, std::mem::take(&mut self.reginfo));
        let func = self.enter_new_func(func_name);
        let len = ast.len();
        let ret = if len == 0 {
            self.new_integer(0)
        } else {
            self.gen_stmts(
                local_map,
                &ast.iter()
                    .map(|(expr, span)| (Stmt::Expr((expr.clone(), span.clone())), span.clone()))
                    .collect::<Vec<(Stmt, Span)>>(),
            )?
        };
        let ty = self[ret].ty;
        self.new_ret(ret);
        self.functions[func].ret = Some(ret);
        self.functions[func].ret_ty = Some(ty);
        self.functions[func].register_num = self.register_num();
        (self.cur_fn, self.cur_bb, self.reginfo) = save;
        Ok(func)
    }

    /// Generate HIR from [(Stmt, Span)].
    fn gen_stmts(
        &mut self,
        local_map: &mut HashMap<String, (usize, Type)>,
        ast: &[(Stmt, Span)],
    ) -> Result<SsaReg> {
        let len = ast.len();
        for (node, _) in &ast[..len - 1] {
            match node {
                Stmt::Expr(expr) => self.gen_nouse(local_map, &expr.0)?,
                Stmt::Decl(decl) => self.gen_decl_nouse(&decl.0)?,
            }
        }
        match &ast[len - 1].0 {
            Stmt::Expr(expr) => self.gen(local_map, &expr.0),
            Stmt::Decl(decl) => self.gen_decl(&decl.0),
        }
    }

    /// Generate HIR from an *Expr*.
    fn gen(
        &mut self,
        local_map: &mut HashMap<String, (usize, Type)>,
        ast: &Expr,
    ) -> Result<SsaReg> {
        match ast {
            Expr::Integer(i) => Ok(self.new_integer(*i)),
            Expr::Float(f) => Ok(self.new_float(*f)),
            Expr::Neg(box (lhs, _)) => {
                match lhs {
                    Expr::Integer(i) => return Ok(self.new_integer(-i)),
                    Expr::Float(f) => return Ok(self.new_float(-f)),
                    _ => {}
                };
                let lhs_i = self.gen(local_map, lhs)?;
                let ssa = match self[lhs_i].ty {
                    Type::Integer => self.new_ineg(lhs_i),
                    Type::Float => self.new_fneg(lhs_i),
                    ty => return Err(HirErr::TypeMismatch(ty, ty)),
                };
                Ok(ssa)
            }
            Expr::Add(box lhs, box rhs) => {
                binary_ops!(self, local_map, lhs, rhs, new_iadd, new_fadd)
            }
            Expr::Sub(box lhs, box rhs) => {
                binary_ops!(self, local_map, lhs, rhs, new_isub, new_fsub)
            }
            Expr::Cmp(kind, box (lhs, _), box (rhs, _)) => match (lhs, rhs) {
                (Expr::Integer(lhs_), Expr::Integer(rhs_)) => Ok(self.new_icmp(
                    *kind,
                    HirOperand::integer(*lhs_),
                    HirOperand::integer(*rhs_),
                )),
                (Expr::Integer(lhs_), _) => {
                    let rhs = self.gen(local_map, rhs)?;
                    let rhs_ty = self[rhs].ty;
                    match rhs_ty {
                        Type::Integer => Ok(self.new_icmp(
                            *kind,
                            HirOperand::integer(*lhs_),
                            HirOperand::reg(rhs),
                        )),
                        Type::Float => {
                            let lhs = self.new_as_float_imm(*lhs_);
                            Ok(self.new_fcmp(*kind, lhs, rhs))
                        }
                        ty => Err(HirErr::TypeMismatch(ty, rhs_ty)),
                    }
                }
                (_, Expr::Integer(rhs_)) => {
                    let lhs = self.gen(local_map, lhs)?;
                    let lhs_ty = self[lhs].ty;
                    match lhs_ty {
                        Type::Integer => Ok(self.new_icmp(
                            *kind,
                            HirOperand::reg(lhs),
                            HirOperand::integer(*rhs_),
                        )),
                        Type::Float => {
                            let rhs = self.new_as_float_imm(*rhs_);
                            Ok(self.new_fcmp(*kind, lhs, rhs))
                        }
                        ty => Err(HirErr::TypeMismatch(ty, Type::Integer)),
                    }
                }
                _ => {
                    let lhs = self.gen(local_map, lhs)?;
                    let rhs = self.gen(local_map, rhs)?;
                    let lhs_ty = self[lhs].ty;
                    let rhs_ty = self[rhs].ty;
                    match (lhs_ty, rhs_ty) {
                        (Type::Integer, Type::Integer) => {
                            Ok(self.new_icmp(*kind, HirOperand::Reg(lhs), HirOperand::Reg(rhs)))
                        }
                        (Type::Integer, Type::Float) => {
                            let lhs = self.new_as_float(lhs);
                            Ok(self.new_fcmp(*kind, lhs, rhs))
                        }
                        (Type::Float, Type::Integer) => {
                            let rhs = self.new_as_float(rhs);
                            Ok(self.new_fcmp(*kind, lhs, rhs))
                        }
                        (Type::Float, Type::Float) => Ok(self.new_fcmp(*kind, lhs, rhs)),
                        (ty_l, ty_r) => Err(HirErr::TypeMismatch(ty_l, ty_r)),
                    }
                }
            },
            Expr::Mul(box (lhs, _), box (rhs, _)) => {
                let lhs = self.gen(local_map, lhs)?;
                let rhs = self.gen(local_map, rhs)?;
                let lhs_ty = self[lhs].ty;
                let rhs_ty = self[rhs].ty;
                match (lhs_ty, rhs_ty) {
                    (Type::Integer, Type::Integer) => Ok(self.new_imul(lhs, rhs)),
                    (Type::Integer, Type::Float) => {
                        let lhs = self.new_as_float(lhs);
                        Ok(self.new_fmul(HirOperand::Reg(lhs), HirOperand::Reg(rhs)))
                    }
                    (Type::Float, Type::Integer) => {
                        let rhs = self.new_as_float(rhs);
                        Ok(self.new_fmul(HirOperand::Reg(lhs), HirOperand::Reg(rhs)))
                    }
                    (Type::Float, Type::Float) => {
                        Ok(self.new_fmul(HirOperand::Reg(lhs), HirOperand::Reg(rhs)))
                    }
                    (ty_l, ty_r) => Err(HirErr::TypeMismatch(ty_l, ty_r)),
                }
            }
            Expr::Div(box (lhs, _), box (rhs, _)) => {
                let lhs = self.gen(local_map, lhs)?;
                let rhs = self.gen(local_map, rhs)?;
                let lhs_ty = self[lhs].ty;
                let rhs_ty = self[rhs].ty;
                match (lhs_ty, rhs_ty) {
                    (Type::Integer, Type::Integer) => Ok(self.new_idiv(lhs, rhs)),
                    (Type::Integer, Type::Float) => {
                        let lhs = self.new_as_float(lhs);
                        Ok(self.new_fdiv(HirOperand::Reg(lhs), HirOperand::Reg(rhs)))
                    }
                    (Type::Float, Type::Integer) => {
                        let rhs = self.new_as_float(rhs);
                        Ok(self.new_fdiv(HirOperand::Reg(lhs), HirOperand::Reg(rhs)))
                    }
                    (Type::Float, Type::Float) => {
                        Ok(self.new_fdiv(HirOperand::Reg(lhs), HirOperand::Reg(rhs)))
                    }
                    (ty_l, ty_r) => Err(HirErr::TypeMismatch(ty_l, ty_r)),
                }
            }
            Expr::LocalStore(ident, box (rhs, _)) => {
                let rhs = self.gen(local_map, rhs)?;
                self.new_local_store(local_map, ident, rhs)
            }
            Expr::LocalLoad(ident) => self.new_local_load(local_map, ident),
            Expr::If(box (cond_, _), box (then_, _), box (else_, _)) => {
                let else_bb = self.new_bb();
                let then_bb = self.new_bb();
                let succ_bb = self.new_bb();
                if let Expr::Cmp(kind, box (lhs, _), box (rhs, _)) = cond_ {
                    let lhs = self.gen(local_map, lhs)?;
                    let lhs_ty = self[lhs].ty;
                    if let Expr::Integer(rhs) = rhs {
                        match lhs_ty {
                            Type::Integer => {
                                self.insts.push(Hir::ICmpBr(
                                    *kind,
                                    lhs,
                                    HirOperand::Const(Value::Integer(*rhs)),
                                    then_bb,
                                    else_bb,
                                ));
                            }
                            Type::Float => {
                                let rhs = self.new_as_float_imm(*rhs);
                                self.insts
                                    .push(Hir::FCmpBr(*kind, lhs, rhs, then_bb, else_bb));
                            }
                            _ => return Err(HirErr::TypeMismatch(lhs_ty, Type::Integer)),
                        };
                    } else {
                        let rhs = self.gen(local_map, rhs)?;
                        let rhs_ty = self[rhs].ty;
                        match (lhs_ty, rhs_ty) {
                            (Type::Integer, Type::Integer) => {
                                self.insts.push(Hir::ICmpBr(
                                    *kind,
                                    lhs,
                                    HirOperand::Reg(rhs),
                                    then_bb,
                                    else_bb,
                                ));
                            }
                            (Type::Float, Type::Float) => {
                                self.insts
                                    .push(Hir::FCmpBr(*kind, lhs, rhs, then_bb, else_bb));
                            }
                            (Type::Integer, Type::Float) => {
                                let lhs = self.new_as_float(lhs);
                                self.insts
                                    .push(Hir::FCmpBr(*kind, lhs, rhs, then_bb, else_bb));
                            }
                            (Type::Float, Type::Integer) => {
                                let rhs = self.new_as_float(rhs);
                                self.insts
                                    .push(Hir::FCmpBr(*kind, lhs, rhs, then_bb, else_bb));
                            }
                            (ty_l, ty_r) => return Err(HirErr::TypeMismatch(ty_l, ty_r)),
                        };
                    }
                } else {
                    let cond_ = self.gen(local_map, cond_)?;
                    match self[cond_].ty {
                        Type::Bool => {}
                        ty => return Err(HirErr::TypeMismatch(ty, Type::Bool)),
                    };
                    self.insts.push(Hir::CondBr(cond_, then_bb, else_bb));
                }

                self.cur_bb = else_bb;
                let else_ = self.gen(local_map, else_)?;
                let else_bb = self.cur_bb;
                self.insts.push(Hir::Br(succ_bb));

                self.cur_bb = then_bb;
                let then_ = self.gen(local_map, then_)?;
                let then_bb = self.cur_bb;
                self.insts.push(Hir::Br(succ_bb));

                if self[then_].ty != self[else_].ty {
                    return Err(HirErr::TypeMismatch(self[then_].ty, self[else_].ty));
                }
                self.cur_bb = succ_bb;
                let ret = self.new_phi(vec![(then_bb, then_), (else_bb, else_)]);
                Ok(ret)
            }
        }
    }

    /// Generate HIR from an *Expr*.
    fn gen_nouse(
        &mut self,
        local_map: &mut HashMap<String, (usize, Type)>,
        ast: &Expr,
    ) -> Result<()> {
        match ast {
            Expr::Neg(box (lhs, _)) => {
                match lhs {
                    Expr::Integer(_) | Expr::Float(_) => {}
                    _ => self.gen_nouse(local_map, lhs)?,
                };
            }
            Expr::Add(box (lhs, _), box (rhs, _)) => {
                self.gen_nouse(local_map, lhs)?;
                self.gen_nouse(local_map, rhs)?;
            }
            Expr::Sub(box (lhs, _), box (rhs, _)) => {
                self.gen_nouse(local_map, lhs)?;
                self.gen_nouse(local_map, rhs)?;
            }
            Expr::Mul(box (lhs, _), box (rhs, _)) => {
                self.gen_nouse(local_map, lhs)?;
                self.gen_nouse(local_map, rhs)?;
            }
            Expr::Div(box (lhs, _), box (rhs, _)) => {
                self.gen_nouse(local_map, lhs)?;
                self.gen_nouse(local_map, rhs)?;
            }
            Expr::LocalStore(ident, box (rhs, _)) => {
                let rhs = self.gen(local_map, rhs)?;
                self.new_local_store_nouse(local_map, ident, rhs)?;
            }
            Expr::If(box (cond_, _), box (then_, _), box (else_, _)) => {
                let cond_ = self.gen(local_map, cond_)?;
                let then_bb = self.new_bb();
                let else_bb = self.new_bb();
                let succ_bb = self.new_bb();
                self.insts.push(Hir::CondBr(cond_, then_bb, else_bb));
                self.cur_bb = then_bb;
                self.gen_nouse(local_map, then_)?;
                self.insts.push(Hir::Br(succ_bb));
                self.cur_bb = else_bb;
                self.gen_nouse(local_map, else_)?;
                self.insts.push(Hir::Br(succ_bb));
                self.cur_bb = succ_bb;
            }
            _ => {}
        };
        Ok(())
    }

    fn gen_decl(&mut self, decl: &Decl) -> Result<SsaReg> {
        self.gen_decl_nouse(decl)?;
        Ok(self.new_integer(0))
    }

    fn gen_decl_nouse(&mut self, decl: &Decl) -> Result<()> {
        match decl {
            Decl::MethodDef(name, arg_name, body) => {
                let mut local_map = HashMap::default();
                let func = self.new_func_from_ast(name.to_string(), &mut local_map, body)?;
                Ok(())
            }
        }
    }
}
