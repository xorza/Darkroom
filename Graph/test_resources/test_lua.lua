function sum(a, b)
    return a + b
end

sum_info = {
    name = "sum",
    inputs = {
        { "a", "f64" },
        { "b", "f64" }
    },
    outputs = {
        { "result", "f64" }
    }    ,
    func = sum
}

add_function(sum_info)
