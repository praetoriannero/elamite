from lark import Transformer, v_args

import elamite.elx_types as et


class Builder(Transformer):
    NULL = str
    MUT = str
    CONST = str
    STATIC = str
    STRING = str

    def ident(self, ident):
        return et.Identifier(str(ident[0]))

    def integer(self, integer):
        value = str(integer[0])
        return et.Integer(value)

    def float(self, float):
        value = str(float[0])
        return et.Float(value)

    def string_concat(self, string):
        return et.String(string[0])

    def mut(self, mut):
        return "mut"

    def bool_true(self, atom):
        return et.Bool(True)

    def bool_false(self, atom):
        return et.Bool(False)

    def stmt(self, stmt):
        return stmt[0]

    @v_args(meta=True)
    def enum_def(self, meta, enum_def):
        raise NotImplementedError("Parsing for enums currently not supported")

    @v_args(meta=True)
    def import_stmt(self, meta, stmt):
        raise NotImplementedError(
            "Parsing for import statements currently not supported"
        )

    # namespacing
    @v_args(meta=True)
    def module_stmt(self, meta, module):
        ident = module[0]
        types = {
            symbol[0].ident.name: symbol[0]
            for symbol in module[1:]
            if isinstance(symbol[0], et.StructDef)
        }
        funcs = {
            symbol[0].ident.name: symbol[0]
            for symbol in module[1:]
            if isinstance(symbol[0], et.FuncDef)
        }
        globals = {
            symbol[0].ident.name: symbol[0]
            for symbol in module[1:]
            if isinstance(symbol[0], et.GlobalDef)
        }
        modules = {
            symbol[0].ident.name: symbol[0]
            for symbol in module[1:]
            if isinstance(symbol[0], et.Module)
        }
        imports = {
            symbol[0].ident.name: symbol[0]
            for symbol in module[1:]
            if isinstance(symbol[0], et.Import)
        }

        return et.Module(meta, ident, types, funcs, globals, modules, imports)

    # block statements
    @v_args(meta=True)
    def expr_stmt(self, meta, stmt):
        return et.ExprStmt(meta, *stmt)

    @v_args(meta=True)
    def assign_stmt(self, meta, assign_stmt):
        lhs, op, rhs = assign_stmt
        return et.AssignStmt(meta, op, lhs, rhs)

    @v_args(meta=True)
    def let_stmt(self, meta, stmt):
        idx = 0
        mut = False
        if "mut" == stmt[idx]:
            mut = True
            idx += 1

        ident = stmt[idx]
        idx += 1

        if isinstance(stmt[idx], et.Type):
            type = stmt[idx]
            idx += 1
            expr = stmt[idx]
        else:
            expr = stmt[idx]
            type = None  # determined during analysis

        return et.LetStmt(meta, ident, type, expr, mut)

    @v_args(meta=True)
    def for_stmt(self, meta, stmt):
        return et.ForStmt(meta, stmt[0], stmt[1], stmt[2])

    # conditionals
    @v_args(meta=True)
    def while_stmt(self, meta, stmt):
        return et.WhileStmt(meta, stmt[0], stmt[1])

    @v_args(meta=True)
    def if_stmt(self, meta, stmt):
        expr = stmt[0]
        block = stmt[1]
        elif_clauses = [clause for clause in stmt if isinstance(clause, et.ElifClause)]
        if isinstance(stmt[-1], et.ElseClause):
            else_clause = stmt[-1]
        else:
            else_clause = None

        return et.IfStmt(meta, expr, block, elif_clauses, else_clause)

    @v_args(meta=True)
    def elif_clause(self, meta, clause):
        return et.ElifClause(meta, clause[0], clause[1])

    @v_args(meta=True)
    def else_clause(self, meta, clause):
        return et.ElseClause(meta, clause[0])

    # control flow
    @v_args(meta=True)
    def break_stmt(self, meta, stmt):
        # TODO: handle ident case
        return et.BreakStmt(meta, None)

    @v_args(meta=True)
    def continue_stmt(self, meta, stmt):
        # TODO: handle ident case
        return et.ContinueStmt(meta, None)

    @v_args(meta=True)
    def return_stmt(self, meta, stmt):
        if len(stmt):
            expr = stmt[0]
        else:
            expr = None

        return et.ReturnStmt(meta, expr)

    def start(self, module):
        return module[0]

    def module(self, symbols):
        # TODO: handle case where a symbol is defined multiple times

        if not symbols:
            return {}

        return {symbol[0].ident.name: symbol[0] for symbol in symbols}

    def fq_ident(self, ident):
        return et.SEP.join([str(i) for i in ident])

    def type(self, type):
        return et.Type(type[0])

    @v_args(meta=True)
    def struct_def(self, meta, struct_def):
        if len(struct_def) == 1:
            ident = struct_def[0]
            fields = []
        else:
            ident, fields = struct_def[0], struct_def[1:]

        return et.StructDef(meta, ident, fields)

    @v_args(meta=True)
    def struct_field(self, meta, struct_field):
        ident, type = struct_field
        return et.StructField(ident, type)

    @v_args(meta=True)
    def func_def(self, meta, func_def):
        ret_type = None
        param_list = []
        if len(func_def) == 3:
            if isinstance(func_def[-2], et.Type):
                ident, ret_type, body = func_def
            else:
                ident, param_list, body = func_def
        elif len(func_def) == 4:
            ident, param_list, ret_type, body = func_def
        else:
            ident, body = func_def

        return et.FuncDef(meta, ident, param_list, ret_type, body)

    def func_param_list(self, params):
        return params

    def func_param(self, func_param):
        ident, type = func_param
        return et.FuncParam(ident, type)

    def postfix_expr(self, expr):
        return et.PostfixExpr(expr[0], expr[1:])

    def prefix_expr(self, expr):
        return et.PrefixExpr(expr[:-1], expr[-1])

    def prefix_plus(self, _):
        return et.Prefix.PLUS

    def prefix_neg(self, _):
        return et.Prefix.NEG

    def prefix_binv(self, _):
        return et.Prefix.BINV

    def prefix_not(self, _):
        return et.Prefix.NOT

    def prefix_ref(self, _):
        return et.Prefix.REF

    def prefix_deref(self, _):
        return et.Prefix.DEREF

    def func_call(self, func_call):
        args = []
        if len(func_call):
            args = func_call[0]
        return et.FuncCall(args)

    def get_attr(self, get_attr):
        return et.GetAttr(get_attr[0])

    def get_slice(self, get_slice):
        return et.GetSlice(get_slice[0])

    def range(self, range):
        range_start = None
        range_end = None
        range_incl = None

        for elem in range:
            if isinstance(elem, et.RangeStart):
                range_start = elem

            if isinstance(elem, et.RangeEnd):
                range_end = elem

            if isinstance(elem, et.RangeInclusive):
                range_incl = elem

        return et.Range(range_start, range_end, range_incl)

    def range_inclusive(self, range_incl):
        return et.RangeInclusive(True)

    def range_start(self, rstart):
        return et.RangeStart(rstart[0])

    def range_end(self, rend):
        return et.RangeEnd(rend[0])

    def arg_list(self, args):
        return args

    @v_args(meta=True)
    def symbol(self, meta, symbol_def):
        return symbol_def

    def block(self, block):
        return block

    @v_args(meta=True)
    def global_def(self, meta, global_def):
        mut = False
        offset = 1
        if "const" in global_def:
            kind = et.GlobalKind.CONST
        else:
            kind = et.GlobalKind.STATIC
            if "mut" in global_def:
                mut = True
                offset += 1

        ident, type, expr = global_def[offset:]
        return et.GlobalDef(meta, ident, kind, mut, type, expr)

    def and_op(self, expr):
        return et.BinaryOp(et.BinOp.AND, *expr)

    def or_op(self, expr):
        return et.BinaryOp(et.BinOp.OR, *expr)

    def eq_op(self, expr):
        return et.BinaryOp(et.BinOp.EQ, *expr)

    def ne_op(self, expr):
        return et.BinaryOp(et.BinOp.NE, *expr)

    def gt_op(self, expr):
        return et.BinaryOp(et.BinOp.GT, *expr)

    def lt_op(self, expr):
        return et.BinaryOp(et.BinOp.LT, *expr)

    def gte_op(self, expr):
        return et.BinaryOp(et.BinOp.GTE, *expr)

    def lte_op(self, expr):
        return et.BinaryOp(et.BinOp.LTE, *expr)

    def bwor_op(self, expr):
        return et.BinaryOp(et.BinOp.BOR, *expr)

    def bwxor_op(self, expr):
        return et.BinaryOp(et.BinOp.BXOR, *expr)

    def bwand_op(self, expr):
        return et.BinaryOp(et.BinOp.BAND, *expr)

    def shl_op(self, expr):
        return et.BinaryOp(et.BinOp.SHL, *expr)

    def shr_op(self, expr):
        return et.BinaryOp(et.BinOp.SHR, *expr)

    def add_op(self, expr):
        return et.BinaryOp(et.BinOp.ADD, *expr)

    def sub_op(self, expr):
        return et.BinaryOp(et.BinOp.SUB, *expr)

    def mul_op(self, expr):
        return et.BinaryOp(et.BinOp.MUL, *expr)

    def div_op(self, expr):
        return et.BinaryOp(et.BinOp.DIV, *expr)

    def mod_op(self, expr):
        return et.BinaryOp(et.BinOp.MOD, *expr)

    def plus_equal(self, assign):
        return et.AsOp.ADDEQ

    def minus_equal(self, assign):
        return et.AsOp.SUBEQ

    def times_equal(self, assign):
        return et.AsOp.MULEQ

    def div_equal(self, assign):
        return et.AsOp.DIVEQ

    def mod_equal(self, assign):
        return et.AsOp.MODEQ

    def shl_equal(self, assign):
        return et.AsOp.SHLEQ

    def shr_equal(self, assign):
        return et.AsOp.SHREQ

    def bwand_equal(self, assign):
        return et.AsOp.ANDEQ

    def bwor_equal(self, assign):
        return et.AsOp.OREQ

    def equal(self, assign):
        return et.AsOp.EQ
