#include "../src/utils/uuid.hpp"
#include "../src/CoreContext.hpp"

#include <graph.pb.h>

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

    auto nodes1 = ctx.get_nodes();
    REQUIRE(nodes1.empty());

    auto funcs = ctx.get_funcs();
    REQUIRE(!funcs.empty());

    auto new_node = ctx.add_node(funcs[0].id);

    auto nodes = ctx.get_nodes();

    REQUIRE(1 == nodes.size());
}

TEST_CASE("remove node", "[context]") {
    auto ctx = Ctx{};

    auto funcs = ctx.get_funcs();
    auto new_node = ctx.add_node(funcs[0].id);
    ctx.remove_node(new_node.id);

    auto nodes = ctx.get_nodes();

    REQUIRE(nodes.empty());
}

TEST_CASE("test proto", "[proto]") {
    graph::Shirt shirt{};
    shirt.add_color("graph::Color::Color_RED");
    shirt.set_size(graph::Shirt_Size::Shirt_Size_MEDIUM);


    auto str = shirt.SerializeAsString();
    std::cout << str << std::endl;


    graph::Shirt new_shirt{};
    new_shirt.ParseFromString(str);
    std::cout << "CoLor" << new_shirt.color(0) << std::endl;
}
