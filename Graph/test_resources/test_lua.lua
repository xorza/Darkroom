function sum(a, b)
    return a + b
end
function mult(a, b)
    return a * b
end
function val0()
    return 4
end
function val1()
    return 4
end
function print_func(message)
    print(message)
end

functions = {
    {
        name = "val0",
        inputs = { },
        outputs = {
            { "result", "i64" }
        },
        func = val0
    },
    {
        name = "val1",
        inputs = { },
        outputs = {
            { "result", "i64" }
        },
        func = val1
    },
    {
        name = "sum",
        inputs = {
            { "a", "i64" },
            { "b", "i64" }
        },
        outputs = {
            { "result", "i64" }
        },
        func = sum
    },
    {
        name = "mult",
        inputs = {
            { "a", "i64" },
            { "b", "i64" }
        },
        outputs = {
            { "result", "i64" }
        },
        func = mult
    },
    {
        name = "print_func",
        inputs = { { "a", "i64" } },
        outputs = { },
        func = print_func
    }
}

function graph()
    local a = val0()
    local b = val1()
    local c_sum_a_b = sum(a, b)
    local d_mult_b_c = mult(b, c_sum_a_b)
    print_func(d_mult_b_c)
end
