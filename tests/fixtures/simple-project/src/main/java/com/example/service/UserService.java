package com.example.service;

import com.example.exception.ValidationException;
import com.example.model.User;
import com.example.repository.UserRepository;

public class UserService {
    private final UserRepository repository;

    public UserService(UserRepository repository) {
        this.repository = repository;
    }

    public User createUser(String name, String email) throws ValidationException {
        if (name == null || name.isEmpty()) {
            throw new ValidationException("Name is required");
        }
        if (email == null || !email.contains("@")) {
            throw new ValidationException("Valid email is required");
        }
        User user = new User(name, email);
        return repository.save(user);
    }

    public User getUser(Long id) {
        User user = repository.findById(id);
        if (user == null) {
            throw new ValidationException("User not found: " + id);
        }
        return user;
    }

    public void deleteUser(Long id) {
        User user = getUser(id);
        repository.deleteById(user.getId());
    }
}
