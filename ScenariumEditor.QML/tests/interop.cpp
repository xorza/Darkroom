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

TEST_CASE("add node", "[context]") {
    auto ctx = Ctx{};

    auto funcs = ctx.get_func_lib();
    REQUIRE(!funcs.empty());

    auto nodes = ctx.get_graph();
    REQUIRE(nodes.empty());

    ctx.add_node(funcs[0].id);

    nodes = ctx.get_graph();
    REQUIRE(1 == nodes.size());
}

TEST_CASE("remove node", "[context]") {
    auto ctx = Ctx{};

    auto funcs = ctx.get_func_lib();
    ctx.add_node(funcs[0].id);

    auto nodes1 = ctx.get_graph();
    ctx.remove_node(nodes1.back().id);

    auto nodes = ctx.get_func_lib();

    REQUIRE(nodes.empty());
}

TEST_CASE("test proto", "[proto]") {

}
