#!/usr/bin/env python3
"""verbose.py - translate descriptive assembly into GNU as RISC-V assembly.

    verbose.py -o <out dir> <file>...

Each input file is written to <out dir> under the same name, with a .asm
extension changed to .s (an included file such as terse.inc keeps its name,
so `.include` finds the translation). Then assemble the output with as.

What it changes, word by word, outside strings and # comments:
  - the instruction at the start of a line (after any labels), using the
    table INSTRUCTIONS: `load_value` becomes `li`, and so on;
  - register names in operands, using REGISTERS: `arg0` becomes `a0`;
  - your own register names, declared on a line of their own with
        register <name> = <register>
    which last until the end of that file.
Directives (words starting with a dot) and macro names (from `.macro`
lines in any of the input files) pass through unchanged.

Every line becomes exactly one line, so the line numbers in as's error
messages are those of your source. Short mnemonics and short register names
(li, a0) are refused, so a file is all one style.
"""

import os
import re
import sys

INSTRUCTIONS = {
    # constants, addresses and copies
    "load_value": "li",
    "load_address": "la",
    "load_upper": "lui",
    "add_upper_to_pc": "auipc",
    "copy": "mv",
    # memory
    "load_word": "lw",
    "load_half_signed": "lh",
    "load_half_unsigned": "lhu",
    "load_byte_signed": "lb",
    "load_byte_unsigned": "lbu",
    "store_word": "sw",
    "store_half": "sh",
    "store_byte": "sb",
    # arithmetic
    "add": "add",
    "add_constant": "addi",
    "subtract": "sub",
    "negate": "neg",
    "multiply": "mul",
    "multiply_high": "mulh",
    "multiply_high_unsigned": "mulhu",
    "multiply_high_signed_unsigned": "mulhsu",
    "divide": "div",
    "divide_unsigned": "divu",
    "remainder": "rem",
    "remainder_unsigned": "remu",
    # bits
    "bitwise_and": "and",
    "bitwise_and_constant": "andi",
    "bitwise_or": "or",
    "bitwise_or_constant": "ori",
    "bitwise_xor": "xor",
    "bitwise_xor_constant": "xori",
    "bitwise_not": "not",
    "shift_left": "sll",
    "shift_left_constant": "slli",
    "shift_right": "srl",
    "shift_right_constant": "srli",
    "shift_right_arithmetic": "sra",
    "shift_right_arithmetic_constant": "srai",
    # comparisons that set a register to 0 or 1
    "set_if_less": "slt",
    "set_if_less_constant": "slti",
    "set_if_less_unsigned": "sltu",
    "set_if_less_unsigned_constant": "sltiu",
    "set_if_zero": "seqz",
    "set_if_not_zero": "snez",
    # branches
    "branch_if_equal": "beq",
    "branch_if_not_equal": "bne",
    "branch_if_less": "blt",
    "branch_if_less_unsigned": "bltu",
    "branch_if_greater": "bgt",
    "branch_if_greater_unsigned": "bgtu",
    "branch_if_less_or_equal": "ble",
    "branch_if_less_or_equal_unsigned": "bleu",
    "branch_if_greater_or_equal": "bge",
    "branch_if_greater_or_equal_unsigned": "bgeu",
    "branch_if_zero": "beqz",
    "branch_if_not_zero": "bnez",
    "branch_if_negative": "bltz",
    "branch_if_not_negative": "bgez",
    "branch_if_positive": "bgtz",
    # jumps and calls
    "jump": "j",
    "jump_to_register": "jr",
    "jump_and_link": "jal",
    "jump_and_link_register": "jalr",
    "call": "call",
    "tail_call": "tail",
    "return": "ret",
    # the rest
    "environment_call": "ecall",
    "breakpoint": "ebreak",
    "no_operation": "nop",
    "fence": "fence",
}

REGISTERS = {
    "zero": "zero",
    "return_address": "ra",
    "stack_pointer": "sp",
    "global_pointer": "gp",
    "thread_pointer": "tp",
    **{f"arg{i}": f"a{i}" for i in range(8)},
    **{f"temp{i}": f"t{i}" for i in range(7)},
    **{f"saved{i}": f"s{i}" for i in range(12)},
}

SHORT_REGISTERS = (
    {"ra", "sp", "gp", "tp", "fp"}
    | {f"x{i}" for i in range(32)}
    | {f"a{i}" for i in range(8)}
    | {f"t{i}" for i in range(7)}
    | {f"s{i}" for i in range(12)}
)

# A string or character literal, kept as it is.
LITERAL = re.compile(r'"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])\'?')
# A name not preceded by a dot, a backslash (a macro parameter) or a word
# character.
NAME = re.compile(r"(?<![\w.\\$])[A-Za-z_][\w$]*")
# Any labels at the start of a line, then the first word.
LABELS = re.compile(r"\s*(?:[\w.$]+:\s*)*")
WORD = re.compile(r"([A-Za-z_.][\w.$]*)(.*)$", re.S)
REGISTER_LINE = re.compile(r"^\s*register\s+([A-Za-z_]\w*)\s*=\s*([A-Za-z_]\w*)\s*$")
MACRO_LINE = re.compile(r"^\s*\.macro\s+([A-Za-z_][\w$]*)")


def split_comment(line):
    """Returns the code and the comment (from '#' outside a literal on)."""
    pos = 0
    while pos < len(line):
        literal = LITERAL.match(line, pos)
        if literal:
            pos = literal.end()
        elif line[pos] == "#":
            return line[:pos], line[pos:]
        else:
            pos += 1
    return line, ""


def outside_literals(code, replace):
    """Applies replace to the parts of code that are not literals."""
    out, pos = [], 0
    for literal in LITERAL.finditer(code):
        out.append(replace(code[pos:literal.start()]))
        out.append(literal.group())
        pos = literal.end()
    out.append(replace(code[pos:]))
    return "".join(out)


class Translator:
    def __init__(self, macros):
        self.macros = macros
        self.errors = []

    def error(self, where, message):
        self.errors.append(f"{where}: {message}")

    def operands(self, text, aliases, where):
        def name(match):
            word = match.group()
            if word in aliases:
                return aliases[word]
            if word in REGISTERS:
                return REGISTERS[word]
            if word in SHORT_REGISTERS:
                self.error(where, f"short register name '{word}'")
            return word

        return outside_literals(text, lambda part: NAME.sub(name, part))

    def line(self, line, aliases, where):
        code, comment = split_comment(line.rstrip("\n"))
        declared = REGISTER_LINE.match(code)
        if declared:
            alias, register = declared.groups()
            if alias in REGISTERS or alias in INSTRUCTIONS:
                self.error(where, f"'{alias}' is already a name")
            elif register in aliases:
                aliases[alias] = aliases[register]
            elif register in REGISTERS:
                aliases[alias] = REGISTERS[register]
            else:
                self.error(where, f"'{register}' is not a register")
            return f"# register {alias} = {register}{comment and ' ' + comment}"
        labels = LABELS.match(code).group()
        match = WORD.match(code, len(labels))
        if not match:
            return code + comment
        word, rest = match.groups()
        if word.startswith("."):
            return code + comment  # directives pass through
        if word in INSTRUCTIONS:
            word = INSTRUCTIONS[word]
        elif word not in self.macros:
            self.error(where, f"unknown instruction '{word}'")
        return labels + word + self.operands(rest, aliases, where) + comment

    def file(self, path):
        aliases = {}
        with open(path, encoding="utf-8") as f:
            lines = f.readlines()
        return "".join(
            self.line(text, aliases, f"{path}:{number}") + "\n"
            for number, text in enumerate(lines, 1)
        )


def main(argv):
    if len(argv) < 3 or argv[0] != "-o":
        sys.exit(__doc__.strip().split("\n\n")[1])
    out_dir, paths = argv[1], argv[2:]
    macros = set()
    for path in paths:
        with open(path, encoding="utf-8") as f:
            for text in f:
                macro = MACRO_LINE.match(text)
                if macro:
                    macros.add(macro.group(1))
    translator = Translator(macros)
    outputs = [(path, translator.file(path)) for path in paths]
    if translator.errors:
        sys.exit("\n".join(translator.errors))
    os.makedirs(out_dir, exist_ok=True)
    for path, text in outputs:
        base = os.path.basename(path)
        if base.endswith(".asm"):
            base = base[: -len(".asm")] + ".s"
        with open(os.path.join(out_dir, base), "w", encoding="utf-8") as f:
            f.write(text)


if __name__ == "__main__":
    main(sys.argv[1:])
