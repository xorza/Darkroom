function sum(a, b)
    return a + b
end
function mult(a, b)
    return a * b
end
function val1()
    return 4
end
function val2()
    return 9
end
function print_func(message)
    print(message)
end

functions = {
    {
        id = "a937baff-822d-48fd-9154-58751539b59b",
        name = "val2",
        inputs = { },
        outputs = {
            { "result", "int" }
        },
        func = val2
    },
    {
        id = "d4d27137-5a14-437a-8bb5-b2f7be0941a2",
        name = "val1",
        inputs = { },
        outputs = {
            { "result", "int" }
        },
        func = val1
    },
    {
        id = "2d3b389d-7b58-44d9-b3d1-a595765b21a5",
        name = "sum",
        inputs = {
            { "a", "int" },
            { "b", "int" }
        },
        outputs = {
            { "result", "int" }
        },
        func = sum
    },
    {
        id = "432b9bf1-f478-476c-a9c9-9a6e190124fc",
        name = "mult",
        inputs = {
            { "a", "int" },
            { "b", "int" }
        },
        outputs = {
            { "result", "int" }
        },
        func = mult
    },
    {
        id = "f22cd316-1cdf-4a80-b86c-1277acd1408a",
        name = "print_func",
        inputs = { { "a", "int" } },
        outputs = { },
        func = print_func
    }
}

function graph()
    local a = val2()
    local b = val1()
    local c_sum_a_b = sum(a, b)
    local d_mult_b_c = mult(b, c_sum_a_b)
    print_func(d_mult_b_c)
end
