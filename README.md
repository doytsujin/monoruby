# monoruby

[![Rust](https://github.com/sisshiki1969/monoruby/actions/workflows/rust.yml/badge.svg?branch=master)](https://github.com/sisshiki1969/monoruby/actions/workflows/rust.yml)
[![codecov](https://codecov.io/gh/sisshiki1969/monoruby/branch/master/graph/badge.svg?token=vAvpafdKER)](https://codecov.io/gh/sisshiki1969/monoruby)

another toy Ruby implementation with a fast JIT compiler written in Rust.

## Features

- register-based bytecode.
- bytecode executer (virtual machine) written in x86-64 assembly (yes, we currently support only x86-64!).
- a compact and fast JIT compiler. (internally using self-made dynamic assembler [monoasm](https://github.com/sisshiki1969/monoasm))

## Status of this project

This project still remains in early-alpha stage. Currently, only the functionalities described below are implemented.

- classes: Integer, Float, String, Symbol, Class
- superclass and singleton class and methods
- local variables
- if-then-elsif-end statement
- for-in statement
- while statement
- method definition

## Benchmark

- measured by [benchmark-driver](https://github.com/benchmark-driver/benchmark-driver) with '--repeat-count 3' option.
- measurements are shown in iteration/sec (the higher, the better).

|   impl \ bench    |  app_fib  | so_mandelbrot  |  tarai  |
|:-----------------:|----------:|---------------:|--------:|
|  monoruby         |  25.423   |    15.049      |   0.379 |
|  ruby(3.2.0d-ev)  |   3.590   |     0.647      |   0.064 |
|  ruby --yjit      |  14.548   |     0.865      |   0.280 |

## How to run

To build monoruby, You'll need installation of Rust.
Please be aware that **only nightly version of Rust works** for monoruby.

To run ruby program file on monoruby,

```sh
% cargo run app_fib.rb
```

or

```sh
% cargo run --release -- app_fib.rb
```

one liner

```sh
% cargo run -- -e "puts 100"
```

You can launch REPL, omitting file name.

```sh
% cargo run
```
