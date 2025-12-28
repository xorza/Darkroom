function sum(a, b)
    return a + b
end

function mult(a, b)
    return a * b
end

function get_a()
    return 4
end

function get_b()
    return 9
end

function print_func(message)
    print(message)
end

functions = {
    {
        id = "a937baff-822d-48fd-9154-58751539b59b",
        name = "get_b",
        inputs = {},
        outputs = {
            { "result", "int" }
        },
    },
    {
        id = "d4d27137-5a14-437a-8bb5-b2f7be0941a2",
        name = "get_a",
        inputs = {},
        outputs = {
            { "result", "int" }
        },
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
    },
    {
        id = "f22cd316-1cdf-4a80-b86c-1277acd1408a",
        name = "print_func",
        inputs = { { "a", "int" } },
        outputs = {},
    }
}

function graph()
    local a = get_a()                     --4
    local b = get_b()                     --9
    local c_sum_a_b = sum(a, b)           --13
    local d_mult_b_c = mult(b, c_sum_a_b) --117
    print_func(d_mult_b_c)                --117
end
