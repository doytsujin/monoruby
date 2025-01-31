use super::*;

impl Codegen {
    /// Call Method
    ///
    /// ~~~text
    /// MethodCall
    /// +---+---+---+---++---+---+---+---+
    /// | op|ret| name  || class |version|
    /// +---+---+---+---++---+---+---+---+
    /// MethodArgs
    /// +---+---+---+---++---+---+---+---+
    /// | op|rcv|arg|len||   code ptr    |
    /// +---+---+---+---++---+---+---+---+
    ///
    /// operands
    /// ret:  return register
    /// name: method name
    /// rcv:  receiver register
    /// arg:  the start of argument registers
    /// len:  the number of argument registers
    ///
    /// inline method cache
    /// class:    a class of the receiver
    /// version:  class version
    /// code ptr: code pointer of the function
    /// ~~~
    pub(super) fn vm_method_call(&mut self, has_block: bool) -> CodePtr {
        let label = self.jit.get_current_address();
        let exit = self.jit.label();
        let slowpath = self.jit.label();
        let exec = self.jit.label();
        let vm_return = self.vm_return;
        let class_version = self.class_version;
        //
        //      +------+------+------+------+
        //      | MethodCall  |class | ver  |
        //      +------+------+------+------+
        // r13->| MethodArgs  |   CodePtr   |
        //      +------+------+------+------+
        //      |     Meta    |     PC      |
        //      +------+------+------+------+
        //
        // rdi: IdentId
        // r15: %ret
        // [r13 -  8]: class_id
        // [r13 -  4]: class_version
        // [r13 +  0]; len
        // [r13 +  2]; %args
        // [r13 +  4]: %recv
        // [r13 +  8]: CodePtr
        // [r13 + 16]: Meta
        // [r13 + 24]: PC

        monoasm! { self.jit,
            pushq r15;
            pushq r13;
            pushq rdi;
            movzxw rdi, [r13 + 4];
        };
        self.vm_get_rdi();
        monoasm! { self.jit,
            pushq rdi;
            // rsp + 24:[%ret]
            // rsp + 16:[pc]
            // rsp + 08:[method_name:IdentId]
            // rsp + 00:[recv:Value]

            // rdi: receiver: Value
            movq rax, (Value::get_class);
            call rax;
            movl r15, rax;
            cmpl r15, [r13 - 8];
            jne  slowpath;
            movl rdi, [r13 - 4];
            cmpl rdi, [rip + class_version];
            jne  slowpath;

        exec:
        };
        self.push_frame(false);
        monoasm! { self.jit,
            // set meta
            movq rdi, [r13 + 16];
            movq [rsp -(16 + OFFSET_META)], rdi;
            movzxw rcx, [r13 + 2]; // rcx <- args
            movzxw rdi, [r13 + 0];  // rdi <- len
            // set self (= receiver)
            movq rax, [rsp];
            movq [rsp - (16 + OFFSET_SELF)], rax;
        };
        self.vm_get_addr_rcx(); // rcx <- *args

        if has_block {
            // set block
            monoasm! { self.jit,
                movq rax, [rcx];
                movq [rsp - (16 + OFFSET_BLOCK)], rax;
                subq rcx, 8;
            };
        } else {
            monoasm! { self.jit,
                movq [rsp - (16 + OFFSET_BLOCK)], 0;
            };
        }
        // set arguments
        monoasm! { self.jit,
          movq r8, rdi;
        }
        self.set_arguments();
        monoasm! { self.jit,
            // argument registers:
            //   rdi: args len
            //
            // global registers:
            //   rbx: &mut Interp
            //   r12: &mut Globals
            //   r13: pc
            //
            movq rax, [r13 + 8];
            // set pc
            movq r13, [r13 + 24];    // r13: BcPc
            call rax;
        };
        self.pop_frame();
        monoasm! { self.jit,
            addq rsp, 16;
            popq r13;   // pop pc
            popq r15;   // pop %ret
            addq r13, 32;
            testq rax, rax;
            jeq vm_return;
        };
        self.vm_store_r15_if_nonzero(exit);
        self.fetch_and_dispatch();

        self.jit.select_page(1);
        let entry_find_method = self.entry_find_method;
        monoasm!(self.jit,
        slowpath:
            movq rsi, [rsp + 8];  // rsi: IdentId
            movzxw rdx, [r13];  // rdx: len
            movq rcx, [rsp]; // rcx: receiver:Value
            call entry_find_method; // rax <- Option<&FuncData>
            testq rax, rax;
            jeq vm_return;
            movl [r13 - 8], r15;
            movl rdi, [rip + class_version];
            movl [r13 - 4], rdi;
            movq rdi, [rax + (FUNCDATA_OFFSET_CODEPTR)];
            movq [r13 + 8], rdi;
            movq rdi, [rax + (FUNCDATA_OFFSET_META)];
            movq [r13 + 16], rdi;
            movq rdi, [rax + (FUNCDATA_OFFSET_PC)];
            movq [r13 + 24], rdi;
            jmp exec;
        );
        self.jit.select_page(0);

        label
    }

    /// Yield
    ///
    /// ~~~text
    /// Yield
    /// +---+---+---+---++---+---+---+---+
    /// | op|ret|arg|len||               |
    /// +---+---+---+---++---+---+---+---+
    ///
    /// ret:  return register
    /// arg:  the start of argument registers
    /// len:  the number of argument registers
    /// ~~~
    pub(super) fn vm_yield(&mut self) -> CodePtr {
        let label = self.jit.get_current_address();
        let exit = self.jit.label();
        let vm_return = self.vm_return;
        // r15: %ret
        // rdi: %args
        // rsi: len

        monoasm! { self.jit,
            pushq r15;
            pushq r13; // push pc
            // rsp + 08:[%ret]
            // rsp + 00:[pc]
            pushq rdi;
            pushq rsi;
            movq rdi, r12;
            movq rsi, [rbp - (OFFSET_BLOCK)];
            movq rdx, rbx;
            movq rax, (get_block_data);
            call rax;
            // rax <- outer_cfp, rdx <- &FuncData
            popq r10;  // r10 <- len
            popq rcx;  // rcx <- %args
            // r9 <- CodePtr
            movq r9, [rdx + (FUNCDATA_OFFSET_CODEPTR)];
            // set meta
            movq rdi, [rdx + (FUNCDATA_OFFSET_META)];
            movq [rsp -(16 + OFFSET_META)], rdi;
            // set pc
            movq r13, [rdx + (FUNCDATA_OFFSET_PC)];
            // set block
            movq [rsp - (16 + OFFSET_BLOCK)], 0;
        };
        self.push_frame(true);
        self.vm_get_addr_rcx(); // rcx <- *args

        monoasm! { self.jit,
          movq r8, r10;
        }
        self.set_arguments();
        monoasm! { self.jit,
            // argument registers:
            //   rdi: args len
            //
            // global registers:
            //   rbx: &mut Interp
            //   r12: &mut Globals
            //   r13: pc
            //
            movq rdi, r10;
            call r9;
        };
        self.pop_frame();
        monoasm! { self.jit,
            popq r13;   // pop pc
            popq r15;   // pop %ret
            testq rax, rax;
            jeq vm_return;
        };
        self.vm_store_r15_if_nonzero(exit);
        self.fetch_and_dispatch();
        label
    }

    /// Set arguments
    ///
    /// ### in
    ///
    /// - r8: arg len
    /// - rcx: the first argument address
    ///
    fn set_arguments(&mut self) {
        let loop_ = self.jit.label();
        let loop_exit = self.jit.label();
        // set arguments
        monoasm! { self.jit,
            testq r8, r8;
            jeq  loop_exit;
            negq r8;
        loop_:
            movq rax, [rcx + r8 * 8 + 8];
            movq [rsp + r8 * 8 - (16 + OFFSET_SELF)], rax;
            addq r8, 1;
            jne  loop_;
        loop_exit:
        };
    }
}
