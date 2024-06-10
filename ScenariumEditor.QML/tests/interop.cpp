#include "../src/utils/uuid.hpp"
#include "../src/CoreContext.hpp"


#include <example_generated.h>

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

TEST_CASE("test fbs", "[fbs]") {
    // Read the binary file
    std::ifstream file("data.bin", std::ios::binary);
    if (!file) {
        std::cerr << "Failed to open file" << std::endl;
        FAIL();
    }

    // Get the length of the file
    file.seekg(0, std::ios::end);
    size_t length = file.tellg();
    file.seekg(0, std::ios::beg);

    // Read the file content into a buffer
    std::vector<char> buffer(length);
    file.read(buffer.data(), length);

    // Verify the buffer
    auto verifier = flatbuffers::Verifier(reinterpret_cast<const uint8_t*>(buffer.data()), length);
    if (!verifier.VerifyBuffer<Example::OuterStruct>(nullptr)) {
        std::cerr << "Failed to verify buffer" << std::endl;
        FAIL();
    }

    // Access the buffer
    auto outer = flatbuffers::GetRoot<Example::OuterStruct>(buffer.data());

    std::cout << "Name: " << outer->name()->c_str() << "\n";
    auto items = outer->items();
    for (auto item : *items) {
        std::cout << "Item ID: " << item->id() << ", Value: " << item->value()->c_str() << "\n";
    }
}
