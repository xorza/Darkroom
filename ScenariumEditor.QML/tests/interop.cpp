#include "../src/utils/uuid.hpp"
#include "../src/CoreContext.hpp"


#include <iostream>
#include <fstream>
#include <vector>
#include <string>

#include <catch2/catch_test_macros.hpp>


TEST_CASE("Uuid tests", "[uuid]") {
    auto uuid = uuid::new_v4();
    auto uuid_str = uuid.to_string();
    auto uuid2 = uuid::from_string(uuid_str);
    REQUIRE(uuid == uuid2);

    auto uuid_str2 = "b5bd73a8-ee5b-46b8-9464-ed0915dad6d7";
    auto uuid3 = uuid::from_string(uuid_str2);
    REQUIRE(uuid3.to_string() == uuid_str2);
}

TEST_CASE("add/remove node", "[context]") {
    auto ctx = Ctx{};

    auto funcs = ctx.get_func_lib();
    REQUIRE(!funcs.empty());

    auto graph = ctx.get_graph();
    REQUIRE(graph.nodes.empty());

    ctx.add_node(funcs[0].id);

    graph = ctx.get_graph();
    REQUIRE(1 == graph.nodes.size());

    ctx.remove_node(graph.nodes[0].id);

    graph = ctx.get_graph();
    REQUIRE(graph.nodes.empty());
}


TEST_CASE("test proto", "[proto]") {

}
